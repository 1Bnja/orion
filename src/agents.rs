//! Perfiles de agente: una pestaña que lanza un comando (el agente), opcionalmente en un
//! git worktree propio para que varios agentes trabajen en paralelo sin pisarse.

use crate::pty::ShellKind;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Corre git en `dir` y devuelve lo que imprimió, o su mensaje de error.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("no encuentro git ({e})"))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    if out.status.success() {
        Ok(text(&out.stdout))
    } else {
        Err(text(&out.stderr))
    }
}

/// Crea un worktree al lado del repo de `dir`, en una rama nueva: `../repo-agente-N` en `orion/agente-N`.
/// ponytail: corre en el hilo de la ventana (git tarda poco); en repos enormes convendría un hilo aparte.
pub(crate) fn create_worktree(dir: &Path, agent: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?);
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

/// Lo que se escribe en la shell nueva: ir a `dir` (si hay) y lanzar el agente. Una línea por paso,
/// así sirve igual en PowerShell, cmd y WSL.
pub(crate) fn launch_input(shell: &ShellKind, dir: Option<&Path>, command: &str) -> String {
    let cd = dir.map(|d| {
        let quoted = shell.quote_path(&d.to_string_lossy());
        match shell.name {
            "cmd" => format!("cd /d {quoted}\r"),
            "WSL" => format!("cd {quoted}\r"),
            _ => format!("Set-Location -LiteralPath {quoted}\r"),
        }
    });
    format!("{}{command}\r", cd.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::SHELLS;

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

        let _ = std::fs::remove_dir_all(&base);
    }
}
