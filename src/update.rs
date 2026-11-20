//! Actualizador: busca una versión más nueva en GitHub Releases y la instala sin salir de Orion.
//!
//! Usa programas que ya vienen con el sistema, sin librerías de red:
//! - `curl` para preguntar cuál es la última versión y bajar el zip,
//! - `certutil.exe` (Windows) o `shasum` (macOS) para verificar el SHA256 que publica la release,
//! - `tar.exe` (Windows) o `ditto` (macOS) para descomprimir.
//!
//! Lo descargado así no lleva la marca de "descargado de internet" que pone el navegador,
//! así que ni SmartScreen ni Gatekeeper vuelven a avisar al abrir la versión nueva.
//!
//! Qué se reemplaza: en Windows, el orion.exe; en macOS, la Orion.app entera.
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
        // Las releases traen Windows y macOS: en otro sistema no hay qué bajar.
        if cfg!(not(any(windows, target_os = "macos"))) {
            return self.set(State::CheckFailed(
                "en este sistema no se actualiza solo".into(),
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

#[cfg(windows)]
const CURL: &str = "curl.exe";
#[cfg(not(windows))]
const CURL: &str = "curl";
/// Adónde tirar lo que no nos interesa guardar.
#[cfg(windows)]
const NULL: &str = "NUL";
#[cfg(not(windows))]
const NULL: &str = "/dev/null";

/// El archivo de la release para esta plataforma, sin el ".zip".
fn asset_name(version: &str) -> String {
    if cfg!(windows) {
        format!("orion-{version}-windows-x64")
    } else {
        format!("orion-{version}-macos")
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
        quiet(CURL).args(["-sS", "-o", NULL, "-w", "%{redirect_url}", &url]),
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

/// Baja la versión `version` y la pone en lugar de esta.
fn install(version: &str) -> Result<(), String> {
    let current = installed()?; // antes de bajar nada: ¿hay qué reemplazar?
    let new = download(version)?;
    replace(&current, &new)
}

/// Lo que reemplaza una actualización: el orion.exe en Windows; en macOS, la Orion.app que tiene
/// adentro al binario (…/Orion.app/Contents/MacOS/orion).
fn installed() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if cfg!(windows) {
        return Ok(exe);
    }
    app_bundle(&exe)
        .ok_or_else(|| "Orion no está abierto desde Orion.app: no hay qué reemplazar".into())
}

/// ".../Orion.app/Contents/MacOS/orion" → ".../Orion.app" (si de verdad está dentro de una .app).
fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let app = exe.ancestors().nth(3)?;
    (app.extension()? == "app").then(|| app.to_path_buf())
}

/// Baja el zip de `version` y su .sha256, y lo deja verificado y descomprimido.
/// Devuelve dónde quedó lo nuevo (orion.exe u Orion.app).
fn download(version: &str) -> Result<PathBuf, String> {
    let name = asset_name(version);
    let base = format!("https://github.com/{REPO}/releases/download/v{version}/{name}.zip");
    let dir = std::env::temp_dir().join(format!("orion-update-{version}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let zip = dir.join(format!("{name}.zip"));
    let sha = dir.join(format!("{name}.zip.sha256"));

    for (url, dest) in [(base.clone(), &zip), (format!("{base}.sha256"), &sha)] {
        run(
            quiet(CURL).args(["-sSfL", "-o"]).arg(dest).arg(&url),
            "no pude descargar la versión nueva",
        )?;
    }
    unpack(&dir, &name, &zip, &sha)
}

/// Compara el SHA256 de `zip` con el publicado en `sha` y, si coincide, lo descomprime en `dir`.
fn unpack(dir: &Path, name: &str, zip: &Path, sha: &Path) -> Result<PathBuf, String> {
    // El .sha256 dice "HASH  nombre.zip".
    let expected = std::fs::read_to_string(sha).map_err(|e| e.to_string())?;
    let expected = expected.split_whitespace().next().unwrap_or_default();
    let actual = sha256(zip)?;
    if expected.len() != 64 || !expected.eq_ignore_ascii_case(&actual) {
        return Err(
            "la descarga no coincide con la publicada (SHA256 distinto): no instalé nada".into(),
        );
    }

    let mut extract = if cfg!(windows) {
        let mut cmd = quiet("tar.exe");
        cmd.arg("-xf").arg(zip).arg("-C").arg(dir);
        cmd
    } else {
        let mut cmd = quiet("ditto"); // como lo comprimió scripts/package-macos.sh
        cmd.args(["-x", "-k"]).arg(zip).arg(dir);
        cmd
    };
    run(&mut extract, "no pude descomprimir")?;

    let (new, binary) = if cfg!(windows) {
        let exe = dir.join(name).join("orion.exe");
        (exe.clone(), exe)
    } else {
        let app = dir.join("Orion.app");
        (app.clone(), app.join("Contents/MacOS/orion"))
    };
    if !binary.is_file() {
        let what = new.file_name().unwrap_or_default().to_string_lossy();
        return Err(format!("el zip no trae {what}"));
    }
    Ok(new)
}

/// El SHA256 de `file`, en hexadecimal.
#[cfg(windows)]
fn sha256(file: &Path) -> Result<String, String> {
    // certutil imprime: una línea de título, el hash (a veces con espacios) y "CertUtil: …".
    let out = run(
        quiet("certutil.exe")
            .arg("-hashfile")
            .arg(file)
            .arg("SHA256"),
        "no pude verificar la descarga",
    )?;
    Ok(out
        .lines()
        .nth(1)
        .unwrap_or_default()
        .split_whitespace()
        .collect())
}

/// El SHA256 de `file`, en hexadecimal.
#[cfg(not(windows))]
fn sha256(file: &Path) -> Result<String, String> {
    // shasum imprime "HASH  ruta".
    let out = run(
        quiet("shasum").args(["-a", "256"]).arg(file),
        "no pude verificar la descarga",
    )?;
    Ok(out.split_whitespace().next().unwrap_or_default().into())
}

/// La versión vieja, al lado de la actual (orion.old.exe u Orion.old.app). Se borra al abrir la nueva.
fn old_path(current: &Path) -> PathBuf {
    current.with_extension(if cfg!(windows) { "old.exe" } else { "old.app" })
}

/// Borra un archivo o una carpeta (una .app es una carpeta), si existe.
fn remove(path: &Path) {
    let _ = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
}

/// Windows no deja sobrescribir un .exe que está corriendo, pero sí renombrarlo; macOS tampoco se
/// queja si se renombra la .app abierta. Así que la actual pasa a ser la "old" y la nueva toma su
/// nombre. Si algo falla, se deshace.
fn replace(current: &Path, new: &Path) -> Result<(), String> {
    let old = old_path(current);
    remove(&old);
    std::fs::rename(current, &old)
        .map_err(|e| format!("no pude apartar la versión actual: {e}"))?;
    if let Err(e) = copy(new, current) {
        remove(current); // lo que haya quedado a medias
        let _ = std::fs::rename(&old, current);
        return Err(format!("no pude copiar la versión nueva: {e}"));
    }
    Ok(())
}

/// Copia lo nuevo a su lugar. En macOS, `ditto` copia la .app entera (con sus permisos y su firma),
/// aunque la carpeta temporal esté en otro disco que /Applications.
fn copy(from: &Path, to: &Path) -> Result<(), String> {
    if cfg!(windows) {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        run(quiet("ditto").arg(from).arg(to), "ditto").map(|_| ())
    }
}

/// Al arrancar: borra la versión vieja que dejó una actualización (ya no está corriendo).
pub(crate) fn clean_up() {
    if let Ok(current) = installed() {
        remove(&old_path(&current));
    }
}

/// Abre Orion (ya actualizado) de nuevo. La app se cierra después.
pub(crate) fn restart() -> Result<(), String> {
    let current = installed()?;
    let mut cmd = if cfg!(windows) {
        Command::new(&current)
    } else {
        // `open -n`: una instancia nueva aunque esta siga abierta unos instantes más.
        let mut cmd = Command::new("open");
        cmd.arg("-n").arg(&current);
        cmd
    };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
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

    #[test]
    fn finds_the_app_bundle() {
        let exe = Path::new("/Applications/Orion.app/Contents/MacOS/orion");
        assert_eq!(
            app_bundle(exe).as_deref(),
            Some(Path::new("/Applications/Orion.app"))
        );
        assert_eq!(app_bundle(Path::new("/repo/target/debug/orion")), None); // cargo run
        assert_eq!(app_bundle(Path::new("orion")), None);
    }

    /// Contra GitHub de verdad (necesita internet): `cargo test real_release -- --ignored`.
    #[test]
    #[ignore]
    fn real_release_downloads_and_verifies() {
        let latest = latest_version().unwrap();
        assert!(parse(&latest).is_some(), "{latest}");
        let new = download(&latest).unwrap();
        assert!(new.exists(), "{}", new.display());
    }

    /// Una carpeta de prueba para cada test (corren en paralelo).
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orion-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Una "instalación" de mentira: el .exe en Windows; en macOS, una .app con su binario adentro.
    fn binary(install: &Path) -> PathBuf {
        if cfg!(windows) {
            install.to_path_buf()
        } else {
            install.join("Contents/MacOS/orion")
        }
    }

    fn fake_install(install: &Path, text: &str) {
        let file = binary(install);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    #[test]
    fn replacing_keeps_the_old_version_aside() {
        let dir = test_dir("replace");
        let name = if cfg!(windows) {
            "orion.exe"
        } else {
            "Orion.app"
        };
        let (current, new) = (dir.join(name), dir.join("nueva").join(name));
        fake_install(&current, "viejo");
        fake_install(&new, "nuevo");
        replace(&current, &new).unwrap();
        let read = |install: &Path| std::fs::read_to_string(binary(install)).unwrap();
        assert_eq!(read(&current), "nuevo");
        assert_eq!(read(&old_path(&current)), "viejo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_of_a_known_file() {
        let dir = test_dir("sha");
        let file = dir.join("abc.txt");
        std::fs::write(&file, "abc").unwrap();
        assert_eq!(
            sha256(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// El zip de macOS, armado como lo arma scripts/package-macos.sh: se verifica y se descomprime.
    #[test]
    #[cfg(target_os = "macos")]
    fn unpacks_only_a_verified_zip() {
        let dir = test_dir("unpack");
        let app = dir.join("build").join("Orion.app");
        fake_install(&app, "nuevo");
        let zip = dir.join("orion-9.9.9-macos.zip");
        let mut ditto = Command::new("ditto");
        ditto.args(["-c", "-k", "--keepParent"]).arg(&app).arg(&zip);
        run(&mut ditto, "ditto").unwrap();

        let sha = dir.join("orion-9.9.9-macos.zip.sha256");
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(&sha, format!("{}  x.zip\n", "0".repeat(64))).unwrap();
        assert!(unpack(&out, "orion-9.9.9-macos", &zip, &sha).is_err()); // otro hash: nada
        assert!(!out.join("Orion.app").exists());

        let hash = sha256(&zip).unwrap();
        std::fs::write(&sha, format!("{hash}  orion-9.9.9-macos.zip\n")).unwrap();
        let new = unpack(&out, "orion-9.9.9-macos", &zip, &sha).unwrap();
        assert_eq!(std::fs::read_to_string(binary(&new)).unwrap(), "nuevo");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
