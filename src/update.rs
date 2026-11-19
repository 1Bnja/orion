//! Actualizador: busca una versión más nueva en GitHub Releases y la instala sin salir de Orion.
//!
//! Usa programas que ya vienen con Windows 11, sin librerías de red:
//! - `curl.exe` para preguntar cuál es la última versión y bajar el zip,
//! - `certutil.exe` para verificar el SHA256 que publica la release,
//! - `tar.exe` para descomprimir.
//!
//! Lo descargado así no lleva la marca de "descargado de internet" que pone el navegador,
//! así que SmartScreen no vuelve a avisar al abrir la versión nueva.
//!
//! Todo corre en un hilo aparte (bajar un zip tarda): la UI solo mira `Updater::state`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

const REPO: &str = "1Bnja/orion";
/// La versión de este .exe (sale de Cargo.toml al compilar).
pub(crate) const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum State {
    Idle,
    Checking,
    UpToDate,
    /// Hay una versión más nueva (ej. "0.2.0").
    Available(String),
    Installing(String),
    /// Ya está el .exe nuevo en su lugar: falta reiniciar.
    Ready,
    /// No se pudo preguntar (ej. sin internet).
    CheckFailed(String),
    /// Falló la instalación (no se cambió nada).
    Failed(String),
}

#[derive(Clone)]
pub(crate) struct Updater {
    state: Arc<Mutex<State>>,
}

impl Updater {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::Idle)),
        }
    }

    pub(crate) fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    fn set(&self, state: State) {
        *self.state.lock().unwrap() = state;
    }

    /// Ya se avisó del resultado: volver a reposo.
    pub(crate) fn dismiss(&self) {
        self.set(State::Idle);
    }

    /// Pregunta en segundo plano si hay una versión nueva.
    pub(crate) fn check(&self, ctx: &eframe::egui::Context) {
        if matches!(self.state(), State::Checking | State::Installing(_)) {
            return;
        }
        // ponytail: el release de macOS (y su actualizador) llega con el issue #48.
        if cfg!(not(windows)) {
            return self.set(State::CheckFailed(
                "en macOS todavía no se actualiza solo".into(),
            ));
        }
        self.set(State::Checking);
        let (this, ctx) = (self.clone(), ctx.clone());
        std::thread::spawn(move || {
            let state = match latest_version() {
                Ok(latest) if is_newer(&latest, CURRENT) => State::Available(latest),
                Ok(_) => State::UpToDate,
                Err(e) => State::CheckFailed(e),
            };
            this.set(state);
            ctx.request_repaint();
        });
    }

    /// Baja e instala `version` en segundo plano. Al terminar queda en `Ready` (o `Failed`).
    pub(crate) fn install(&self, ctx: &eframe::egui::Context, version: String) {
        self.set(State::Installing(version.clone()));
        let (this, ctx) = (self.clone(), ctx.clone());
        std::thread::spawn(move || {
            let state = match install(&version) {
                Ok(()) => State::Ready,
                Err(e) => State::Failed(e),
            };
            this.set(state);
            ctx.request_repaint();
        });
    }
}

/// Un programa sin que aparezca una consola negra (en Windows; Orion es una app de ventana).
pub(crate) fn quiet(program: &str) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Corre el comando y devuelve lo que imprimió, o un error legible.
fn run(cmd: &mut Command, what: &str) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{what}: {}", err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// La última versión publicada. GitHub redirige /releases/latest a /releases/tag/vX.Y.Z:
/// leemos ese destino sin bajar la página (ni usar la API, que limita las consultas).
fn latest_version() -> Result<String, String> {
    let url = format!("https://github.com/{REPO}/releases/latest");
    let redirect = run(
        quiet("curl.exe").args(["-sS", "-o", "NUL", "-w", "%{redirect_url}", &url]),
        "no pude consultar GitHub",
    )?;
    version_from_url(&redirect).ok_or_else(|| "GitHub no dijo cuál es la última versión".into())
}

/// ".../releases/tag/v0.2.0" → "0.2.0".
fn version_from_url(url: &str) -> Option<String> {
    let tag = url
        .rsplit("/tag/")
        .next()
        .filter(|_| url.contains("/tag/"))?;
    let version = tag.trim().strip_prefix('v')?;
    parse(version).map(|_| version.to_string())
}

fn parse(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.').map(|p| p.parse().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// ¿`latest` es más nueva que `current`? (comparando números: 0.10.0 > 0.9.0).
fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse(latest), parse(current)), (Some(l), Some(c)) if l > c)
}

/// Baja la versión `version` y pone su .exe en lugar de este.
fn install(version: &str) -> Result<(), String> {
    let new_exe = download(version)?;
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    replace_exe(&current, &new_exe)
}

/// Baja el zip de `version`, verifica su SHA256 y lo descomprime. Devuelve dónde quedó el orion.exe nuevo.
fn download(version: &str) -> Result<PathBuf, String> {
    let name = format!("orion-{version}-windows-x64");
    let base = format!("https://github.com/{REPO}/releases/download/v{version}/{name}.zip");
    let dir = std::env::temp_dir().join(format!("orion-update-{version}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let zip = dir.join(format!("{name}.zip"));
    let sha = dir.join(format!("{name}.zip.sha256"));

    for (url, dest) in [(base.clone(), &zip), (format!("{base}.sha256"), &sha)] {
        run(
            quiet("curl.exe").args(["-sSfL", "-o"]).arg(dest).arg(&url),
            "no pude descargar la versión nueva",
        )?;
    }

    // El .sha256 dice "HASH  nombre.zip"; comparamos con el hash del zip que bajamos.
    let expected = std::fs::read_to_string(&sha).map_err(|e| e.to_string())?;
    let expected = expected.split_whitespace().next().unwrap_or_default();
    // certutil imprime: una línea de título, el hash (a veces con espacios) y "CertUtil: …".
    let out = run(
        quiet("certutil.exe")
            .arg("-hashfile")
            .arg(&zip)
            .arg("SHA256"),
        "no pude verificar la descarga",
    )?;
    let actual: String = out
        .lines()
        .nth(1)
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if !expected.eq_ignore_ascii_case(&actual) {
        return Err(
            "la descarga no coincide con la publicada (SHA256 distinto): no instalé nada".into(),
        );
    }

    run(
        quiet("tar.exe").arg("-xf").arg(&zip).arg("-C").arg(&dir),
        "no pude descomprimir",
    )?;
    let new_exe = dir.join(&name).join("orion.exe");
    if !new_exe.is_file() {
        return Err("el zip no trae orion.exe".into());
    }
    Ok(new_exe)
}

/// El .exe viejo, al lado del actual. Se borra al abrir la versión nueva.
fn old_exe(current: &Path) -> PathBuf {
    current.with_extension("old.exe")
}

/// Windows no deja borrar ni sobrescribir un .exe que está corriendo, pero sí renombrarlo:
/// el actual pasa a orion.old.exe y el nuevo toma su nombre. Si algo falla, se deshace.
fn replace_exe(current: &Path, new_exe: &Path) -> Result<(), String> {
    let old = old_exe(current);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(current, &old).map_err(|e| format!("no pude apartar el .exe actual: {e}"))?;
    if let Err(e) = std::fs::copy(new_exe, current) {
        let _ = std::fs::rename(&old, current);
        return Err(format!("no pude copiar el .exe nuevo: {e}"));
    }
    Ok(())
}

/// Al arrancar: borra el .exe viejo que dejó una actualización (ya no está corriendo).
pub(crate) fn clean_up() {
    if let Ok(current) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_exe(&current));
    }
}

/// Abre el .exe (ya actualizado) de nuevo. La app se cierra después.
pub(crate) fn restart() -> Result<(), String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(current)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        let url = "https://github.com/1Bnja/orion/releases/tag/v0.2.0";
        assert_eq!(version_from_url(url).as_deref(), Some("0.2.0"));
        assert_eq!(
            version_from_url("https://github.com/1Bnja/orion/releases"),
            None
        );
        assert!(is_newer("0.2.0", "0.1.1"));
        assert!(is_newer("0.10.0", "0.9.9")); // números, no texto
        assert!(!is_newer("0.1.1", "0.1.1"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("basura", "0.1.1"));
    }

    /// Contra GitHub de verdad (necesita internet): `cargo test real_release -- --ignored`.
    #[test]
    #[ignore]
    fn real_release_downloads_and_verifies() {
        let latest = latest_version().unwrap();
        assert!(parse(&latest).is_some(), "{latest}");
        let exe = download("0.1.1").unwrap();
        assert!(std::fs::metadata(&exe).unwrap().len() > 1_000_000);
    }

    #[test]
    fn replacing_the_exe_keeps_the_old_one_aside() {
        let dir = std::env::temp_dir().join(format!("orion-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (current, new) = (dir.join("orion.exe"), dir.join("nuevo.exe"));
        std::fs::write(&current, "viejo").unwrap();
        std::fs::write(&new, "nuevo").unwrap();
        replace_exe(&current, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&current).unwrap(), "nuevo");
        assert_eq!(std::fs::read_to_string(old_exe(&current)).unwrap(), "viejo");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
