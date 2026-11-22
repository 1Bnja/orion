//! Perfiles de agente: una pestaña que lanza un comando (el agente), opcionalmente en un
//! git worktree propio para que varios agentes trabajen en paralelo sin pisarse.

use crate::pty::ShellKind;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Un CLI de agente que Orion sabe abrir. El perfil `[[agent]]` sigue siendo la vía libre para
/// cualquier otro comando; estos son los que ya vienen listos y se detectan solos.
pub(crate) struct Provider {
    /// Nombre para el menú y la paleta.
    pub(crate) name: &'static str,
    /// Programa base; en Windows se prueban `.exe`, `.cmd` y `.bat`.
    binary: &'static str,
    /// Lo que se escribe en la shell para abrirlo.
    pub(crate) command: &'static str,
    /// Una línea para el tooltip: quién lo hace y cómo instalarlo.
    pub(crate) about: &'static str,
}

/// Los agentes que Orion reconoce sin configurar nada. Para cualquier otro, un `[[agent]]`.
pub(crate) static PROVIDERS: [Provider; 5] = [
    Provider {
        name: "Claude Code",
        binary: "claude",
        command: "claude",
        about: "Anthropic · code.claude.com",
    },
    Provider {
        name: "Gemini CLI",
        binary: "gemini",
        command: "gemini",
        about: "Google · npm i -g @google/gemini-cli",
    },
    Provider {
        name: "Codex",
        binary: "codex",
        command: "codex",
        about: "OpenAI · npm i -g @openai/codex",
    },
    Provider {
        name: "OpenCode",
        binary: "opencode",
        command: "opencode",
        about: "open source · opencode.ai",
    },
    Provider {
        name: "Aider",
        binary: "aider",
        command: "aider",
        about: "cualquier modelo, incluido DeepSeek · aider.chat",
    },
];

/// Carpetas donde buscar los CLIs: el PATH y donde los dejan los instaladores.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    // En Windows no hay HOME: la carpeta personal es USERPROFILE.
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    if let Some(home) = home {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".npm-global/bin"));
        dirs.push(home.join(".bun/bin"));
    }
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    dirs
}

/// Los nombres que puede tener el ejecutable en este sistema.
fn candidates(base: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        vec![
            format!("{base}.exe"),
            format!("{base}.cmd"),
            format!("{base}.bat"),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![base.to_string()]
    }
}

/// El ejecutable de `base` en la primera carpeta que lo tenga (mismo orden que el PATH).
fn find_binary(base: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    candidates(base)
        .into_iter()
        .flat_map(|name| dirs.iter().map(move |d| d.join(&name)))
        .find(|p| p.is_file())
}

/// Los índices de `PROVIDERS` que están instalados. Se calcula una vez por sesión.
pub(crate) fn installed() -> &'static [usize] {
    static INSTALLED: OnceLock<Vec<usize>> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let dirs = search_dirs();
        PROVIDERS
            .iter()
            .enumerate()
            .filter(|(_, p)| find_binary(p.binary, &dirs).is_some())
            .map(|(i, _)| i)
            .collect()
    })
}

/// Corre git en `dir` y devuelve lo que imprimió, o su mensaje de error.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = crate::update::quiet("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("no encuentro git ({e})"))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    if out.status.success() {
        Ok(text(&out.stdout))
    } else {
        Err(text(&out.stderr))
    }
}

/// La carpeta raíz del repo git que contiene a `dir` (error si no está en uno).
pub(crate) fn repo_root(dir: &Path) -> Result<PathBuf, String> {
    git(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// Crea un worktree al lado del repo de `dir`, en una rama nueva: `../repo-agente-N` en `orion/agente-N`.
/// ponytail: corre en el hilo de la ventana (git tarda poco); en repos enormes convendría un hilo aparte.
pub(crate) fn create_worktree(dir: &Path, agent: &str) -> Result<PathBuf, String> {
    let root = repo_root(dir)?;
    let repo = root
        .file_name()
        .ok_or("el repo no tiene nombre")?
        .to_string_lossy();
    let slug: String = agent
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let parent = root.parent().ok_or("el repo está en la raíz del disco")?;
    for n in 1.. {
        let path = parent.join(format!("{repo}-{slug}-{n}"));
        let branch = format!("orion/{slug}-{n}");
        let taken = git(
            &root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        );
        if path.exists() || taken.is_ok() {
            continue;
        }
        let target = path.to_string_lossy();
        git(&root, &["worktree", "add", "-b", &branch, &target])?;
        return Ok(path);
    }
    unreachable!()
}

/// Los worktrees que Orion creó en `repo` (los de ramas `orion/…`): su carpeta y su rama.
pub(crate) fn orion_worktrees(repo: &Path) -> Vec<(PathBuf, String)> {
    let Ok(text) = git(repo, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    let (mut found, mut path, mut branch) = (Vec::new(), None, None);
    // Un bloque por worktree, separados por una línea vacía (la última lleva el que cierra).
    for line in text.lines().chain([""]) {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            branch = Some(b.to_string());
        } else if line.is_empty()
            && let (Some(p), Some(b)) = (path.take(), branch.take())
            && b.starts_with("orion/")
        {
            found.push((p, b));
        }
    }
    found
}

/// Quita un worktree y su rama. Sin `--force`: si hay cambios sin guardar, falla y no toca nada.
/// Devuelve si también se borró la rama (si tiene commits sin fusionar, se conserva).
pub(crate) fn remove_worktree(repo: &Path, path: &Path, branch: &str) -> Result<bool, String> {
    git(repo, &["worktree", "remove", &path.to_string_lossy()])?;
    Ok(git(repo, &["branch", "-d", branch]).is_ok())
}

/// Lo que se escribe en la shell nueva: ir a `dir` (si hay) y lanzar el agente. Una línea por paso,
/// así sirve igual en PowerShell, cmd y WSL.
pub(crate) fn launch_input(shell: &ShellKind, dir: Option<&Path>, command: &str) -> String {
    let cd = dir.map(|d| {
        let quoted = shell.quote_path(&d.to_string_lossy());
        if shell.powershell {
            format!("Set-Location -LiteralPath {quoted}\r")
        } else if shell.name == "cmd" {
            format!("cd /d {quoted}\r") // /d: también cambia de disco
        } else {
            format!("cd {quoted}\r") // WSL, zsh, bash, fish
        }
    });
    // Sin comando (una terminal sola en esa carpeta) no hace falta el Enter de más.
    let run = if command.is_empty() {
        String::new()
    } else {
        format!("{command}\r")
    };
    format!("{}{run}", cd.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::SHELLS;

    #[cfg(windows)]
    #[test]
    fn launch_goes_to_the_folder_first() {
        let dir = Some(Path::new(r"C:\repo agente"));
        assert_eq!(
            launch_input(&SHELLS[0], dir, "mi-agente"),
            "Set-Location -LiteralPath 'C:\\repo agente'\rmi-agente\r"
        );
        assert_eq!(
            launch_input(&SHELLS[2], dir, "a"),
            "cd /d \"C:\\repo agente\"\ra\r"
        );
        assert_eq!(
            launch_input(&SHELLS[3], dir, "a"),
            "cd '/mnt/c/repo agente'\ra\r"
        );
        assert_eq!(launch_input(&SHELLS[0], None, "a"), "a\r");
    }

    #[cfg(not(windows))]
    #[test]
    fn launch_goes_to_the_folder_first_on_mac() {
        let dir = Some(Path::new("/Users/ana/mi repo"));
        let zsh = &SHELLS[0];
        assert_eq!(launch_input(zsh, dir, "a"), "cd '/Users/ana/mi repo'\ra\r");
    }

    #[test]
    fn a_provider_is_found_in_the_first_folder_that_has_it() {
        let base = std::env::temp_dir().join(format!("orion-prov-{}", std::process::id()));
        let (a, b) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let name = candidates("foo")[0].clone();
        std::fs::write(b.join(&name), "").unwrap();
        assert_eq!(
            find_binary("foo", &[a.clone(), b.clone()]),
            Some(b.join(&name))
        );
        assert_eq!(find_binary("nope", &[a, b]), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn every_provider_has_a_unique_name() {
        let mut names: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PROVIDERS.len());
    }

    #[test]
    fn worktrees_get_their_own_folder_and_branch() {
        let base = std::env::temp_dir().join(format!("orion-wt-{}", std::process::id()));
        let repo = base.join("proyecto");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| git(&repo, args).unwrap();
        run(&["init", "-q"]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);

        let first = create_worktree(&repo, "Mi Agente").unwrap();
        let second = create_worktree(&repo.join("."), "Mi Agente").unwrap();
        assert_eq!(first.file_name().unwrap(), "proyecto-mi-agente-1");
        assert_eq!(second.file_name().unwrap(), "proyecto-mi-agente-2");
        assert_eq!(
            git(&second, &["branch", "--show-current"]).unwrap(),
            "orion/mi-agente-2"
        );
        // Fuera de un repo, un error claro en vez de un worktree a medias.
        assert!(create_worktree(&std::env::temp_dir(), "x").is_err());

        // Limpieza: los dos worktrees se listan; el que tiene cambios sin guardar se conserva.
        assert_eq!(orion_worktrees(&repo).len(), 2);
        std::fs::write(second.join("nuevo.txt"), "sin guardar").unwrap();
        assert!(remove_worktree(&repo, &second, "orion/mi-agente-2").is_err());
        assert!(second.exists());
        assert_eq!(
            remove_worktree(&repo, &first, "orion/mi-agente-1"),
            Ok(true)
        );
        assert!(!first.exists());
        assert_eq!(orion_worktrees(&repo).len(), 1);

        let _ = std::fs::remove_dir_all(&base);
    }
}
