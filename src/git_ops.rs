//! Operaciones de git para la pestaña "Grafo". Devuelven `Ok` con lo que imprimió git
//! o `Err` con su mensaje de error.
// Todavía nada llama a este módulo: lo conecta la pestaña Grafo.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::git_graph::{FileChange, joined};

/// Una operación a medias que espera `continue_op` o `abort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InProgress {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

/// Qué tanto deshace `reset`: solo HEAD, también el índice, o también el árbol de trabajo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

/// Cómo integra `pull` lo que baja.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullMode {
    FastForward,
    Rebase,
}

/// El estado del árbol de trabajo. Los archivos en conflicto solo salen en `conflicted`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkStatus {
    pub staged: Vec<FileChange>,
    /// Incluye los no rastreados (sin cuenta de líneas).
    pub unstaged: Vec<FileChange>,
    pub conflicted: Vec<String>,
    pub in_progress: Option<InProgress>,
}

/// Un `git` en `repo` que nunca se queda esperando: ni pide claves ni abre un editor.
fn command(repo: &Path, args: &[&str]) -> Command {
    let mut cmd = crate::update::quiet("git");
    cmd.arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .stdin(Stdio::null());
    cmd
}

/// Corre el comando; devuelve (stdout, stderr) tal cual, o el mensaje de error.
fn output(mut cmd: Command, what: &str) -> Result<(String, String), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("no pude ejecutar {what} ({e})"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if out.status.success() {
        return Ok((stdout, stderr));
    }
    // Los conflictos se cuentan por stdout: se juntan los dos.
    let message = join(&stdout, &stderr);
    Err(if message.is_empty() {
        format!("{what} falló sin dar un mensaje ({})", out.status)
    } else {
        message
    })
}

fn join(stdout: &str, stderr: &str) -> String {
    [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Corre git y devuelve todo lo que imprimió, para mostrarlo.
fn run(repo: &Path, args: &[&str]) -> Result<String, String> {
    output(command(repo, args), "git").map(|(out, err)| join(&out, &err))
}

/// Corre git y devuelve solo stdout, sin recortar, para interpretarlo.
fn read(repo: &Path, args: &[&str]) -> Result<String, String> {
    output(command(repo, args), "git").map(|(out, _)| out)
}

/// Rechaza nombres y revs vacíos o que git podría leer como opción.
fn check(names: &[&str]) -> Result<(), String> {
    match names.iter().find(|n| n.is_empty() || n.starts_with('-')) {
        Some(name) => Err(format!("nombre no válido: «{name}»")),
        None => Ok(()),
    }
}

/// La ruta tal cual (sin comodines), relativa a la raíz del repo.
fn pathspec(path: &str) -> String {
    format!(":(top,literal){path}")
}

/// Corre `args -- rutas`. Sin rutas es un error: git lo tomaría como "todo".
fn run_paths(repo: &Path, args: &[&str], paths: &[String]) -> Result<String, String> {
    if paths.is_empty() || paths.iter().any(|p| p.is_empty()) {
        return Err("no hay rutas".to_string());
    }
    let specs: Vec<String> = paths.iter().map(|p| pathspec(p)).collect();
    let mut all = args.to_vec();
    all.push("--");
    all.extend(specs.iter().map(String::as_str));
    run(repo, &all)
}

pub fn status(repo: &Path) -> Result<WorkStatus, String> {
    let changes = |cached: bool| -> Result<Vec<FileChange>, String> {
        let mut args = vec![
            "--no-optional-locks",
            "diff",
            "--numstat",
            "--no-renames",
            "-z",
        ];
        if cached {
            args.push("--cached");
        }
        args.push("--");
        Ok(read(repo, &args)?
            .split('\0')
            .filter_map(|record| {
                let mut fields = record.splitn(3, '\t');
                let added = fields.next()?;
                let removed = fields.next()?;
                let path = fields.next()?;
                Some(FileChange {
                    path: path.to_string(),
                    // En binarios numstat pone `-`, que no parsea: queda `None`.
                    added: added.parse().ok(),
                    removed: removed.parse().ok(),
                })
            })
            .collect())
    };
    // Todas las consultas a la vez: cada `git` cuesta mucho más en arrancar que en responder.
    let (unmerged, staged, unstaged, untracked, in_progress) = std::thread::scope(|s| {
        // Cada registro de `ls-files -u` es `modo hash etapa\truta`; hay uno por etapa.
        let unmerged = s.spawn(|| read(repo, &["ls-files", "-u", "--full-name", "-z", "--", ":/"]));
        let staged = s.spawn(|| changes(true));
        let unstaged = s.spawn(|| changes(false));
        let untracked = s.spawn(|| {
            let args = ["ls-files", "--others", "--exclude-standard", "--full-name"];
            read(repo, &[&args[..], &["-z", "--", ":/"]].concat())
        });
        let in_progress = in_progress(repo);
        (
            joined(unmerged),
            joined(staged),
            joined(unstaged),
            joined(untracked),
            in_progress,
        )
    });
    let mut conflicted: Vec<String> = unmerged?
        .split('\0')
        .filter_map(|record| record.split_once('\t'))
        .map(|(_, path)| path.to_string())
        .collect();
    conflicted.dedup();
    // Los archivos en conflicto van aparte.
    let (mut staged, mut unstaged) = (staged?, unstaged?);
    staged.retain(|file| !conflicted.contains(&file.path));
    unstaged.retain(|file| !conflicted.contains(&file.path));
    unstaged.extend(
        untracked?
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(|path| FileChange {
                path: path.to_string(),
                added: None,
                removed: None,
            }),
    );

    Ok(WorkStatus {
        staged,
        unstaged,
        conflicted,
        in_progress: in_progress?,
    })
}

fn in_progress(repo: &Path) -> Result<Option<InProgress>, String> {
    let dir = read(repo, &["rev-parse", "--absolute-git-dir"])?;
    let dir = Path::new(dir.trim_end_matches(['\r', '\n']));
    // El rebase va primero: mientras dura puede dejar también las marcas de los otros.
    Ok([
        ("rebase-merge", InProgress::Rebase),
        ("rebase-apply", InProgress::Rebase),
        ("MERGE_HEAD", InProgress::Merge),
        ("CHERRY_PICK_HEAD", InProgress::CherryPick),
        ("REVERT_HEAD", InProgress::Revert),
    ]
    .into_iter()
    .find(|(mark, _)| dir.join(mark).exists())
    .map(|(_, op)| op))
}

/// Más líneas que estas no se muestran: un diff enorme no se lee y pesa en cada frame.
pub const MAX_DIFF_LINES: usize = 20_000;

/// Los cambios sin commitear de un archivo, tal cual los da git (sin recortar, para poder
/// aplicarlos por bloques): lo preparado (`staged`) o lo que falta preparar. De un archivo
/// sin rastrear se muestra todo su contenido como agregado; de un binario, nada.
pub fn wip_diff(repo: &Path, path: &str, staged: bool) -> Result<String, String> {
    let spec = pathspec(path);
    // La configuración del usuario no puede cambiar el formato: `git apply` lo necesita así.
    let mut args = vec![
        "--no-optional-locks",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", &spec]);
    let diff = read(repo, &args)?;
    if !diff.is_empty() || staged {
        return Ok(diff);
    }
    let others = [
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
        "--",
        &spec,
    ];
    if read(repo, &others)?.is_empty() {
        return Ok(diff);
    }
    let root = read(repo, &["rev-parse", "--show-toplevel"])?;
    let file = Path::new(root.trim_end_matches(['\r', '\n'])).join(path);
    // ponytail: se lee a lo sumo 2 MB; más que eso no entra en `MAX_DIFF_LINES`.
    let mut bytes = Vec::new();
    std::fs::File::open(&file)
        .and_then(|f| std::io::Read::read_to_end(&mut std::io::Read::take(f, 2 << 20), &mut bytes))
        .map_err(|e| format!("no pude leer {}: {e}", file.display()))?;
    if bytes.contains(&0) {
        return Ok(String::new());
    }
    let text = String::from_utf8_lossy(&bytes);
    let added: String = text.lines().map(|line| format!("+{line}\n")).collect();
    Ok(format!("@@ archivo nuevo @@\n{added}"))
}

/// El parche de un solo bloque de `diff` (el de un archivo): su cabecera y el bloque que
/// empieza en la línea `start`. None si ahí no empieza un bloque que se pueda aplicar.
pub fn hunk_patch(diff: &[String], start: usize) -> Option<String> {
    let first = diff.iter().position(|line| line.starts_with("@@"))?;
    // Sin cabecera (archivo nuevo) o con `@@@` (conflicto) no hay parche que aplicar.
    if first == 0 || !diff.get(start)?.starts_with("@@ ") {
        return None;
    }
    let end = diff[start + 1..]
        .iter()
        .position(|line| line.starts_with("@@"))
        .map_or(diff.len(), |n| start + 1 + n);
    let lines = diff[..first].iter().chain(&diff[start..end]);
    Some(lines.map(|line| format!("{line}\n")).collect())
}

/// Prepara un bloque (`hunk_patch`) o, con `reverse`, lo quita de lo preparado. El árbol de
/// trabajo no se toca.
pub fn apply_hunk(repo: &Path, patch: &str, reverse: bool) -> Result<String, String> {
    let mut args = vec!["apply", "--cached", "--whitespace=nowarn"];
    if reverse {
        args.push("--reverse");
    }
    args.push("-");
    let mut cmd = command(repo, &args);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let error = |e: std::io::Error| format!("no pude ejecutar git ({e})");
    let mut child = cmd.spawn().map_err(error)?;
    // Soltar el stdin al terminar de escribir es lo que le dice a git que no hay más.
    if let Some(mut stdin) = child.stdin.take() {
        std::io::Write::write_all(&mut stdin, patch.as_bytes()).map_err(error)?;
    }
    let out = child.wait_with_output().map_err(error)?;
    let message = join(
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    );
    if out.status.success() {
        Ok(message)
    } else {
        Err(message)
    }
}

pub fn stage(repo: &Path, paths: &[String]) -> Result<String, String> {
    run_paths(repo, &["add", "-A"], paths)
}

pub fn unstage(repo: &Path, paths: &[String]) -> Result<String, String> {
    // `reset` (y no `restore --staged`) también sirve en un repo sin commits.
    run_paths(repo, &["reset", "-q"], paths)
}

/// Descarta lo no preparado: los rastreados vuelven a como están en el índice y los
/// no rastreados se borran (los ignorados no se tocan).
pub fn discard(repo: &Path, paths: &[String]) -> Result<String, String> {
    // `restore` falla con rutas que git no conoce, así que se separan antes.
    let mut tracked = Vec::new();
    for path in paths {
        if !read(repo, &["ls-files", "-z", "--", &pathspec(path)])?.is_empty() {
            tracked.push(path.clone());
        }
    }
    let mut text = String::new();
    if !tracked.is_empty() {
        text = run_paths(repo, &["restore", "--worktree"], &tracked)?;
    }
    let cleaned = run_paths(repo, &["clean", "-f", "-d"], paths)?;
    Ok(join(&text, &cleaned))
}

pub fn commit(repo: &Path, message: &str, amend: bool) -> Result<String, String> {
    let mut args = vec!["commit", "-m", message];
    if amend {
        args.push("--amend");
    }
    run(repo, &args)
}

pub fn checkout(repo: &Path, rev: &str) -> Result<String, String> {
    check(&[rev])?;
    run(repo, &["checkout", "--end-of-options", rev, "--"])
}

pub fn create_branch(repo: &Path, name: &str, at: &str, checkout: bool) -> Result<String, String> {
    check(&[name, at])?;
    if checkout {
        run(
            repo,
            &["checkout", "-b", name, "--end-of-options", at, "--"],
        )
    } else {
        run(repo, &["branch", "--end-of-options", name, at])
    }
}

pub fn rename_branch(repo: &Path, old: &str, new: &str) -> Result<String, String> {
    check(&[old, new])?;
    run(repo, &["branch", "-m", "--end-of-options", old, new])
}

pub fn delete_branch(repo: &Path, name: &str, force: bool) -> Result<String, String> {
    check(&[name])?;
    let flag = if force { "-D" } else { "-d" };
    run(repo, &["branch", flag, "--end-of-options", name])
}

/// Crea la rama local que sigue a `remote_branch` (`origin/x`) y se cambia a ella.
pub fn track(repo: &Path, remote_branch: &str) -> Result<String, String> {
    check(&[remote_branch])?;
    run(
        repo,
        &[
            "checkout",
            "--track",
            "--end-of-options",
            remote_branch,
            "--",
        ],
    )
}

pub fn merge(repo: &Path, rev: &str) -> Result<String, String> {
    check(&[rev])?;
    run(repo, &["merge", "--no-edit", "--end-of-options", rev])
}

pub fn rebase(repo: &Path, onto: &str) -> Result<String, String> {
    check(&[onto])?;
    run(repo, &["rebase", "--end-of-options", onto])
}

pub fn cherry_pick(repo: &Path, id: &str) -> Result<String, String> {
    check(&[id])?;
    run(repo, &["cherry-pick", "--end-of-options", id])
}

pub fn revert(repo: &Path, id: &str) -> Result<String, String> {
    check(&[id])?;
    run(repo, &["revert", "--no-edit", "--end-of-options", id])
}

pub fn reset(repo: &Path, id: &str, mode: ResetMode) -> Result<String, String> {
    check(&[id])?;
    let flag = match mode {
        ResetMode::Soft => "--soft",
        ResetMode::Mixed => "--mixed",
        ResetMode::Hard => "--hard",
    };
    run(repo, &["reset", flag, "--end-of-options", id, "--"])
}

/// Guarda los cambios, incluidos los archivos no rastreados.
pub fn stash_push(repo: &Path, message: Option<&str>) -> Result<String, String> {
    let mut args = vec!["stash", "push", "--include-untracked"];
    if let Some(message) = message.filter(|m| !m.is_empty()) {
        args.extend(["-m", message]);
    }
    run(repo, &args)
}

fn stash(repo: &Path, action: &str, index: usize) -> Result<String, String> {
    run(repo, &["stash", action, &format!("stash@{{{index}}}")])
}

/// Error si `stash@{index}` ya no es el commit `hash`: los números se corren cuando alguien
/// guarda o borra un stash, y la pila la comparten todos los worktrees.
/// ponytail: queda un instante entre esta comprobación y la operación; git no ofrece hacer
/// ambas de una vez por hash.
pub fn stash_check(repo: &Path, index: usize, hash: &str) -> Result<(), String> {
    let now = read(
        repo,
        &["rev-parse", "--verify", "-q", &format!("stash@{{{index}}}")],
    );
    match now {
        Ok(now) if now.trim() == hash => Ok(()),
        _ => Err("la pila de stashes cambió: vuelve a elegir el stash".to_string()),
    }
}

pub fn stash_apply(repo: &Path, index: usize) -> Result<String, String> {
    stash(repo, "apply", index)
}

pub fn stash_pop(repo: &Path, index: usize) -> Result<String, String> {
    stash(repo, "pop", index)
}

pub fn stash_drop(repo: &Path, index: usize) -> Result<String, String> {
    stash(repo, "drop", index)
}

pub fn create_tag(repo: &Path, name: &str, at: &str) -> Result<String, String> {
    check(&[name, at])?;
    run(repo, &["tag", "--end-of-options", name, at])
}

pub fn delete_tag(repo: &Path, name: &str) -> Result<String, String> {
    check(&[name])?;
    run(repo, &["tag", "-d", "--end-of-options", name])
}

pub fn fetch(repo: &Path) -> Result<String, String> {
    run(repo, &["fetch", "--all", "--prune"])
}

pub fn pull(repo: &Path, mode: PullMode) -> Result<String, String> {
    // Las opciones explícitas mandan sobre `pull.rebase` y `pull.ff` del usuario.
    match mode {
        PullMode::FastForward => run(repo, &["pull", "--no-rebase", "--ff-only"]),
        PullMode::Rebase => run(repo, &["pull", "--rebase"]),
    }
}

/// Sube la rama a su upstream; si no tiene, a `origin` (o al primer remoto) con `-u`.
pub fn push(repo: &Path, branch: &str, force_with_lease: bool) -> Result<String, String> {
    check(&[branch])?;
    let local = format!("refs/heads/{branch}");
    read(repo, &["rev-parse", "--verify", "--end-of-options", &local])
        .map_err(|_| format!("no existe la rama «{branch}»"))?;
    let upstream = read(
        repo,
        &[
            "for-each-ref",
            "--format=%(upstream:remotename)%00%(upstream:remoteref)",
            &local,
        ],
    )?;

    let mut args = vec!["push"];
    if force_with_lease {
        args.push("--force-with-lease");
    }
    let (remote, target) = match upstream.trim_end().split_once('\0') {
        // El remoto "." es un upstream local: ahí no hay nada que subir.
        Some((remote, target)) if !target.is_empty() && remote != "." => {
            (remote.to_string(), target.to_string())
        }
        _ => {
            let remotes = read(repo, &["remote"])?;
            let remote = remotes
                .lines()
                .find(|r| *r == "origin")
                .or_else(|| remotes.lines().next())
                .ok_or("el repo no tiene ningún remoto")?;
            args.push("-u");
            (remote.to_string(), local.clone())
        }
    };
    let refspec = format!("{local}:{target}");
    args.extend(["--end-of-options", &remote, &refspec]);
    run(repo, &args)
}

/// Borra en el remoto la rama `remote_branch` (`origin/x`).
pub fn delete_remote_branch(repo: &Path, remote_branch: &str) -> Result<String, String> {
    check(&[remote_branch])?;
    // El nombre de un remoto puede tener `/`: se busca el más largo que calce.
    let remotes = read(repo, &["remote"])?;
    let (remote, branch) = remotes
        .lines()
        .filter_map(|remote| {
            let branch = remote_branch.strip_prefix(remote)?.strip_prefix('/')?;
            Some((remote, branch))
        })
        .max_by_key(|(remote, _)| remote.len())
        .ok_or_else(|| format!("«{remote_branch}» no es una rama de un remoto"))?;
    check(&[remote, branch])?;
    let full = format!("refs/heads/{branch}");
    run(
        repo,
        &["push", "--delete", "--end-of-options", remote, &full],
    )
}

fn subcommand(op: InProgress) -> &'static str {
    match op {
        InProgress::Merge => "merge",
        InProgress::Rebase => "rebase",
        InProgress::CherryPick => "cherry-pick",
        InProgress::Revert => "revert",
    }
}

pub fn abort(repo: &Path, op: InProgress) -> Result<String, String> {
    run(repo, &[subcommand(op), "--abort"])
}

pub fn continue_op(repo: &Path, op: InProgress) -> Result<String, String> {
    run(repo, &[subcommand(op), "--continue"])
}

/// Resuelve el conflicto de `path` con una de las dos versiones y la prepara.
/// Como en git, durante un rebase "ours" es la rama de destino y "theirs" el commit propio.
pub fn take(repo: &Path, path: &str, ours: bool) -> Result<String, String> {
    let paths = [path.to_string()];
    let stages = read(repo, &["ls-files", "-u", "-z", "--", &pathspec(path)])?;
    if stages.is_empty() {
        return Err(format!("«{path}» no está en conflicto"));
    }
    let (flag, stage_mark) = if ours {
        ("--ours", " 2\t")
    } else {
        ("--theirs", " 3\t")
    };
    // Si ese lado borró el archivo no hay versión que sacar: se resuelve borrándolo.
    if !stages.contains(stage_mark) {
        return run_paths(repo, &["rm", "-f"], &paths);
    }
    let text = run_paths(repo, &["checkout", flag], &paths)?;
    let added = stage(repo, &paths)?;
    Ok(join(&text, &added))
}

/// Rama local -> (adelante, atrás) respecto a su upstream.
pub fn tracking(repo: &Path) -> Result<HashMap<String, (u32, u32)>, String> {
    let mut cmd = command(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(upstream)%00%(upstream:track,nobracket)",
            "refs/heads",
        ],
    );
    // "ahead" y "behind" se traducen según el idioma del sistema.
    cmd.env("LC_ALL", "C");
    let (text, _) = output(cmd, "git")?;
    Ok(text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let name = fields.next()?.strip_prefix("refs/heads/")?;
            let upstream = fields.next()?;
            let track = fields.next()?;
            // "gone": el upstream configurado ya no existe.
            if upstream.is_empty() || track == "gone" {
                return None;
            }
            let count = |word: &str| {
                track
                    .split(", ")
                    .find_map(|part| part.strip_prefix(word)?.trim().parse().ok())
                    .unwrap_or(0)
            };
            Some((name.to_string(), (count("ahead"), count("behind"))))
        })
        .collect())
}

/// Abre un pull request de la rama actual con `gh`; devuelve su URL.
pub fn create_pr(repo: &Path, title: &str, body: &str, base: &str) -> Result<String, String> {
    check(&[base])?;
    // Sin un `origin` de GitHub no hay PR que crear. Fallar acá es inmediato y evita que `gh`
    // tarde (o se quede esperando red) para terminar diciendo lo mismo.
    let origin = read(repo, &["remote", "get-url", "origin"]).unwrap_or_default();
    if github_slug(origin.trim()).is_none() {
        return Err("origin no es un repo de GitHub".into());
    }
    let mut cmd = crate::update::quiet("gh");
    cmd.current_dir(repo)
        .args(["pr", "create"])
        // Con `=` un texto que empiece con `-` no se confunde con una opción.
        .arg(format!("--title={title}"))
        .arg(format!("--body={body}"))
        .arg(format!("--base={base}"))
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    let (stdout, stderr) = output(cmd, "gh")?;
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("https://"))
        .map(String::from)
        .ok_or_else(|| format!("gh no devolvió la URL: {}", join(&stdout, &stderr)))
}

/// La página de GitHub que compara `base` con `head`; `None` si `origin` no es de GitHub.
pub fn compare_url(repo: &Path, base: &str, head: &str) -> Option<String> {
    let url = read(repo, &["remote", "get-url", "origin"]).ok()?;
    let slug = github_slug(url.trim())?;
    // Lo que una rama puede llevar y una URL interpretaría.
    let encode = |s: &str| {
        s.replace('%', "%25")
            .replace('#', "%23")
            .replace('?', "%3F")
    };
    Some(format!(
        "https://github.com/{slug}/compare/{}...{}",
        encode(base),
        encode(head)
    ))
}

/// `OWNER/REPO` de la URL de un remoto de GitHub (https o ssh).
fn github_slug(url: &str) -> Option<String> {
    let (host, path) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        None => url.split_once(':')?,
    };
    // Fuera el usuario (`git@`) y el puerto.
    let host = host.rsplit('@').next()?.split(':').next()?;
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .then(|| format!("{owner}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Carpeta temporal que se borra sola al terminar la prueba.
    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        crate::agents::git(repo, args).unwrap()
    }

    /// Identidad propia y nada de la configuración del usuario que cambie los resultados.
    fn configure(repo: &Path) {
        for (key, value) in [
            ("user.name", "t"),
            ("user.email", "t@t"),
            ("core.autocrlf", "false"),
            ("commit.gpgsign", "false"),
            ("tag.gpgsign", "false"),
        ] {
            git(repo, &["config", key, value]);
        }
    }

    fn write(repo: &Path, file: &str, text: &str) {
        std::fs::write(repo.join(file), text).unwrap();
    }

    fn text(repo: &Path, file: &str) -> String {
        std::fs::read_to_string(repo.join(file)).unwrap()
    }

    fn add_commit(repo: &Path, file: &str, content: &str, message: &str) {
        write(repo, file, content);
        stage(repo, &paths(&[file])).unwrap();
        commit(repo, message, false).unwrap();
    }

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|p| p.to_string()).collect()
    }

    fn names(files: &[FileChange]) -> Vec<&str> {
        files.iter().map(|f| f.path.as_str()).collect()
    }

    fn head(repo: &Path) -> String {
        git(repo, &["rev-parse", "HEAD"])
    }

    /// Un repo temporal con un commit en `main`.
    fn temp_repo(name: &str) -> (Temp, PathBuf) {
        let base = std::env::temp_dir().join(format!("orion-ops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        configure(&repo);
        add_commit(&repo, "raiz.txt", "raiz\n", "raiz");
        (Temp(base), repo)
    }

    #[test]
    fn stage_unstage_commit_y_amend() {
        let (_temp, repo) = temp_repo("commit");
        write(&repo, "raiz.txt", "raiz\nmas\n");
        write(&repo, "con espacio [1].txt", "a\n");

        let both = paths(&["raiz.txt", "con espacio [1].txt"]);
        stage(&repo, &both).unwrap();
        let now = status(&repo).unwrap();
        assert_eq!(names(&now.staged), vec!["con espacio [1].txt", "raiz.txt"]);
        assert_eq!(now.staged[1].added, Some(1));
        assert!(now.unstaged.is_empty());
        assert_eq!(now.in_progress, None);

        unstage(&repo, &both).unwrap();
        let now = status(&repo).unwrap();
        assert!(now.staged.is_empty());
        assert_eq!(
            names(&now.unstaged),
            vec!["raiz.txt", "con espacio [1].txt"]
        );

        // Sin rutas no se hace nada (git lo tomaría como "todo").
        assert!(stage(&repo, &[]).is_err());
        assert!(discard(&repo, &[]).is_err());

        stage(&repo, &paths(&["raiz.txt"])).unwrap();
        commit(&repo, "-cambio", false).unwrap();
        let previous = git(&repo, &["rev-parse", "HEAD~1"]);
        stage(&repo, &paths(&["con espacio [1].txt"])).unwrap();
        commit(&repo, "enmendado", true).unwrap();
        assert_eq!(git(&repo, &["log", "-1", "--format=%s"]), "enmendado");
        assert_eq!(git(&repo, &["rev-parse", "HEAD~1"]), previous);
        assert_eq!(status(&repo).unwrap(), WorkStatus::default());
        // Sin nada que commitear, el error trae el mensaje de git.
        assert!(!commit(&repo, "vacío", false).unwrap_err().is_empty());
    }

    #[test]
    fn discard_restaura_rastreados_y_borra_nuevos() {
        let (_temp, repo) = temp_repo("discard");
        write(&repo, "raiz.txt", "preparado\n");
        stage(&repo, &paths(&["raiz.txt"])).unwrap();
        write(&repo, "raiz.txt", "suelto\n");
        write(&repo, "nuevo.txt", "n\n");
        write(&repo, "otro.txt", "o\n");

        discard(&repo, &paths(&["raiz.txt", "nuevo.txt"])).unwrap();
        assert_eq!(text(&repo, "raiz.txt"), "preparado\n");
        assert!(!repo.join("nuevo.txt").exists());
        assert!(repo.join("otro.txt").exists());
    }

    #[test]
    fn unstage_en_un_repo_sin_commits() {
        let (temp, _) = temp_repo("unborn");
        let repo = temp.0.join("vacio");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        write(&repo, "a.txt", "a\n");
        stage(&repo, &paths(&["a.txt"])).unwrap();
        assert_eq!(names(&status(&repo).unwrap().staged), vec!["a.txt"]);
        unstage(&repo, &paths(&["a.txt"])).unwrap();
        assert!(status(&repo).unwrap().staged.is_empty());
    }

    #[test]
    fn ramas_crear_renombrar_y_borrar() {
        let (_temp, repo) = temp_repo("branch");
        create_branch(&repo, "quieta", "main", false).unwrap();
        assert_eq!(git(&repo, &["branch", "--show-current"]), "main");
        create_branch(&repo, "nueva", "main", true).unwrap();
        assert_eq!(git(&repo, &["branch", "--show-current"]), "nueva");
        add_commit(&repo, "n.txt", "n\n", "n");
        checkout(&repo, "main").unwrap();

        rename_branch(&repo, "nueva", "otra").unwrap();
        // Sin fusionar: solo se borra a la fuerza.
        assert!(delete_branch(&repo, "otra", false).is_err());
        delete_branch(&repo, "otra", true).unwrap();
        delete_branch(&repo, "quieta", false).unwrap();
        assert_eq!(git(&repo, &["branch", "--format=%(refname:short)"]), "main");

        for bad in ["", "-f", "--help"] {
            assert!(checkout(&repo, bad).is_err());
            assert!(create_branch(&repo, bad, "main", false).is_err());
            assert!(delete_branch(&repo, bad, true).is_err());
        }
    }

    #[test]
    fn diffs_separados_y_preparar_por_bloques() {
        let (_temp, repo) = temp_repo("hunks");
        let lines = |changed: &[usize]| -> String {
            (1..=30)
                .map(|n| match changed.contains(&n) {
                    true => format!("linea {n} cambiada\n"),
                    false => format!("linea {n}\n"),
                })
                .collect()
        };
        write(&repo, "largo.txt", &lines(&[]));
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "largo"]);
        // Dos cambios lejos uno del otro: dos bloques.
        write(&repo, "largo.txt", &lines(&[2, 25]));

        let split = |text: String| -> Vec<String> {
            text.split_terminator('\n').map(String::from).collect()
        };
        let diff = |staged| split(wip_diff(&repo, "largo.txt", staged).unwrap());
        let hunks = |diff: &[String]| -> Vec<usize> {
            (0..diff.len())
                .filter(|&i| diff[i].starts_with("@@ "))
                .collect()
        };
        let unstaged = diff(false);
        assert_eq!(hunks(&unstaged).len(), 2);
        assert!(diff(true).is_empty());
        assert!(hunk_patch(&unstaged, 0).is_none()); // la cabecera no es un bloque

        // Preparar solo el segundo.
        let patch = hunk_patch(&unstaged, hunks(&unstaged)[1]).unwrap();
        apply_hunk(&repo, &patch, false).unwrap();
        let (staged, left) = (diff(true), diff(false));
        assert!(staged.iter().any(|l| l == "+linea 25 cambiada"));
        assert!(!staged.iter().any(|l| l == "+linea 2 cambiada"));
        assert!(left.iter().any(|l| l == "+linea 2 cambiada"));
        assert!(!left.iter().any(|l| l == "+linea 25 cambiada"));
        // Aplicarlo otra vez ya no calza: error, sin tocar nada.
        assert!(apply_hunk(&repo, &patch, false).is_err());

        // Quitarlo: vuelve a estar solo en el árbol de trabajo, que nunca cambió.
        let patch = hunk_patch(&staged, hunks(&staged)[0]).unwrap();
        apply_hunk(&repo, &patch, true).unwrap();
        assert!(diff(true).is_empty());
        assert_eq!(text(&repo, "largo.txt"), lines(&[2, 25]));

        // Sin rastrear: se ve entero y no se aplica por bloques. Binario: nada.
        write(&repo, "nuevo.txt", "uno\ndos\n");
        let new = split(wip_diff(&repo, "nuevo.txt", false).unwrap());
        assert_eq!(new, ["@@ archivo nuevo @@", "+uno", "+dos"]);
        assert!(hunk_patch(&new, 0).is_none());
        std::fs::write(repo.join("n.bin"), [1u8, 0, 2]).unwrap();
        assert_eq!(wip_diff(&repo, "n.bin", false).unwrap(), "");
    }

    #[test]
    fn stash_por_indice() {
        let (_temp, repo) = temp_repo("stash");
        write(&repo, "raiz.txt", "primero\n");
        stash_push(&repo, Some("primero")).unwrap();
        write(&repo, "nuevo.txt", "segundo\n");
        stash_push(&repo, None).unwrap();
        assert!(!repo.join("nuevo.txt").exists());
        let count = || git(&repo, &["stash", "list"]).lines().count();
        assert_eq!(count(), 2);

        // El número solo vale mientras apunte al mismo commit.
        let newest = git(&repo, &["rev-parse", "stash@{0}"]);
        let newest = newest.trim();
        assert!(stash_check(&repo, 0, newest).is_ok());
        assert!(stash_check(&repo, 1, newest).is_err());
        assert!(stash_check(&repo, 2, newest).is_err());

        // El índice 1 es el más viejo.
        stash_apply(&repo, 1).unwrap();
        assert_eq!(text(&repo, "raiz.txt"), "primero\n");
        assert_eq!(count(), 2);
        git(&repo, &["checkout", "--", "raiz.txt"]);

        stash_pop(&repo, 0).unwrap();
        assert_eq!(text(&repo, "nuevo.txt"), "segundo\n");
        assert_eq!(count(), 1);
        stash_drop(&repo, 0).unwrap();
        assert_eq!(count(), 0);
        assert!(stash_drop(&repo, 0).is_err());
    }

    #[test]
    fn tags_crear_y_borrar() {
        let (_temp, repo) = temp_repo("tag");
        let first = head(&repo);
        add_commit(&repo, "a.txt", "a\n", "a");
        create_tag(&repo, "v1", &first).unwrap();
        assert_eq!(git(&repo, &["rev-parse", "v1"]), first);
        assert!(create_tag(&repo, "v1", "HEAD").is_err());
        delete_tag(&repo, "v1").unwrap();
        assert!(git(&repo, &["tag"]).is_empty());
        assert!(create_tag(&repo, "-d", "HEAD").is_err());
    }

    #[test]
    fn reset_en_los_tres_modos() {
        let (_temp, repo) = temp_repo("reset");
        let first = head(&repo);
        add_commit(&repo, "raiz.txt", "nueva\n", "nueva");
        let second = head(&repo);

        for (mode, staged, unstaged, content) in [
            (ResetMode::Soft, 1, 0, "nueva\n"),
            (ResetMode::Mixed, 0, 1, "nueva\n"),
            (ResetMode::Hard, 0, 0, "raiz\n"),
        ] {
            reset(&repo, &second, ResetMode::Hard).unwrap();
            reset(&repo, &first, mode).unwrap();
            assert_eq!(head(&repo), first);
            let now = status(&repo).unwrap();
            assert_eq!((now.staged.len(), now.unstaged.len()), (staged, unstaged));
            assert_eq!(text(&repo, "raiz.txt"), content);
        }
    }

    #[test]
    fn cherry_pick_y_revert() {
        let (_temp, repo) = temp_repo("pick");
        create_branch(&repo, "lateral", "main", true).unwrap();
        add_commit(&repo, "lado.txt", "lado\n", "lado");
        let id = head(&repo);
        checkout(&repo, "main").unwrap();

        cherry_pick(&repo, &id).unwrap();
        assert_eq!(text(&repo, "lado.txt"), "lado\n");
        revert(&repo, "HEAD").unwrap();
        assert!(!repo.join("lado.txt").exists());
        assert_eq!(status(&repo).unwrap(), WorkStatus::default());
    }

    /// `main` y `lateral` cambian la misma línea de `raiz.txt`; queda en `main`.
    fn conflicting_repo(name: &str) -> (Temp, PathBuf) {
        let (temp, repo) = temp_repo(name);
        create_branch(&repo, "lateral", "main", true).unwrap();
        add_commit(&repo, "raiz.txt", "ellos\n", "ellos");
        checkout(&repo, "main").unwrap();
        add_commit(&repo, "raiz.txt", "propio\n", "propio");
        (temp, repo)
    }

    #[test]
    fn merge_con_conflicto_take_y_continue() {
        for (ours, expected) in [(true, "propio\n"), (false, "ellos\n")] {
            let (_temp, repo) = conflicting_repo(&format!("merge-{ours}"));
            let error = merge(&repo, "lateral").unwrap_err();
            assert!(error.contains("CONFLICT"), "{error}");

            let now = status(&repo).unwrap();
            assert_eq!(now.in_progress, Some(InProgress::Merge));
            assert_eq!(now.conflicted, vec!["raiz.txt"]);
            assert!(now.staged.is_empty() && now.unstaged.is_empty());

            take(&repo, "raiz.txt", ours).unwrap();
            assert_eq!(text(&repo, "raiz.txt"), expected);
            assert!(status(&repo).unwrap().conflicted.is_empty());
            continue_op(&repo, InProgress::Merge).unwrap();
            assert_eq!(status(&repo).unwrap(), WorkStatus::default());
            assert_eq!(
                git(&repo, &["rev-list", "--count", "--merges", "HEAD"]),
                "1"
            );
        }
    }

    #[test]
    fn abort_y_las_demas_operaciones_a_medias() {
        let (_temp, repo) = conflicting_repo("abort");
        let before = head(&repo);

        type Start = fn(&Path, &str) -> Result<String, String>;
        for (start, op) in [
            (merge as Start, InProgress::Merge),
            (rebase, InProgress::Rebase),
            (cherry_pick, InProgress::CherryPick),
        ] {
            assert!(start(&repo, "lateral").is_err());
            assert_eq!(status(&repo).unwrap().in_progress, Some(op));
            abort(&repo, op).unwrap();
            assert_eq!(status(&repo).unwrap(), WorkStatus::default());
            assert_eq!(head(&repo), before);
        }

        add_commit(&repo, "raiz.txt", "otra\n", "otra");
        assert!(revert(&repo, &before).is_err());
        assert_eq!(status(&repo).unwrap().in_progress, Some(InProgress::Revert));
        take(&repo, "raiz.txt", false).unwrap();
        continue_op(&repo, InProgress::Revert).unwrap();
        assert_eq!(status(&repo).unwrap(), WorkStatus::default());

        // Un rebase y un cherry-pick resueltos también terminan sin abrir editor.
        for (start, op) in [
            (cherry_pick as Start, InProgress::CherryPick),
            (rebase, InProgress::Rebase),
        ] {
            let (_temp, repo) = conflicting_repo(&format!("continue-{op:?}"));
            assert!(start(&repo, "lateral").is_err());
            // "theirs" es el commit traído en el cherry-pick; en el rebase, el propio.
            take(&repo, "raiz.txt", false).unwrap();
            continue_op(&repo, op).unwrap();
            assert_eq!(status(&repo).unwrap(), WorkStatus::default());
        }
    }

    #[test]
    fn take_borra_si_ese_lado_borro_el_archivo() {
        let (_temp, repo) = temp_repo("take-delete");
        create_branch(&repo, "lateral", "main", true).unwrap();
        git(&repo, &["rm", "-q", "raiz.txt"]);
        commit(&repo, "borra", false).unwrap();
        checkout(&repo, "main").unwrap();
        add_commit(&repo, "raiz.txt", "propio\n", "propio");

        assert!(merge(&repo, "lateral").is_err());
        assert_eq!(status(&repo).unwrap().conflicted, vec!["raiz.txt"]);
        take(&repo, "raiz.txt", false).unwrap();
        assert!(!repo.join("raiz.txt").exists());
        continue_op(&repo, InProgress::Merge).unwrap();
        assert!(take(&repo, "raiz.txt", true).is_err());
    }

    #[test]
    fn push_pull_fetch_contra_un_remoto_bare() {
        let (temp, repo) = temp_repo("remote");
        let bare = temp.0.join("remoto.git");
        let bare_arg = bare.to_str().unwrap();
        git(&temp.0, &["init", "-q", "--bare", bare_arg]);
        git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(&repo, &["remote", "add", "origin", bare_arg]);

        assert!(tracking(&repo).unwrap().is_empty());
        push(&repo, "main", false).unwrap();
        assert_eq!(tracking(&repo).unwrap()["main"], (0, 0));
        add_commit(&repo, "a.txt", "a\n", "a");
        assert_eq!(tracking(&repo).unwrap()["main"], (1, 0));
        push(&repo, "main", false).unwrap();

        let other = temp.0.join("otro");
        git(&temp.0, &["clone", "-q", bare_arg, other.to_str().unwrap()]);
        configure(&other);
        add_commit(&other, "b.txt", "b\n", "b");
        push(&other, "main", false).unwrap();
        fetch(&repo).unwrap();
        assert_eq!(tracking(&repo).unwrap()["main"], (0, 1));
        pull(&repo, PullMode::FastForward).unwrap();
        assert_eq!(head(&repo), head(&other));

        // Con las dos ramas avanzadas, ff-only falla y rebase funciona.
        add_commit(&other, "c.txt", "c\n", "c");
        push(&other, "main", false).unwrap();
        add_commit(&repo, "d.txt", "d\n", "d");
        assert!(push(&repo, "main", false).is_err());
        assert!(pull(&repo, PullMode::FastForward).is_err());
        pull(&repo, PullMode::Rebase).unwrap();
        push(&repo, "main", false).unwrap();

        // Reescribir historia necesita force-with-lease.
        commit(&repo, "d2", true).unwrap();
        assert!(push(&repo, "main", false).is_err());
        push(&repo, "main", true).unwrap();

        // Una rama remota: seguirla y borrarla.
        create_branch(&other, "feature/x", "main", true).unwrap();
        push(&other, "feature/x", false).unwrap();
        fetch(&repo).unwrap();
        track(&repo, "origin/feature/x").unwrap();
        assert_eq!(git(&repo, &["branch", "--show-current"]), "feature/x");
        assert_eq!(tracking(&repo).unwrap()["feature/x"], (0, 0));
        delete_remote_branch(&repo, "origin/feature/x").unwrap();
        assert!(git(&bare, &["branch", "--list", "feature/x"]).is_empty());
        // Sin upstream vivo no aparece.
        assert!(!tracking(&repo).unwrap().contains_key("feature/x"));
        assert!(delete_remote_branch(&repo, "nada/x").is_err());
        assert!(push(&repo, "no-existe", false).is_err());
    }

    #[test]
    fn compare_url_solo_para_github() {
        let (_temp, repo) = temp_repo("compare");
        assert_eq!(compare_url(&repo, "main", "x"), None);
        git(&repo, &["remote", "add", "origin", "x"]);
        let url = |remote: &str| {
            git(&repo, &["remote", "set-url", "origin", remote]);
            compare_url(&repo, "main", "fix/#1")
        };
        let expected = Some("https://github.com/o/r/compare/main...fix/%231".to_string());
        assert_eq!(url("https://github.com/o/r.git"), expected);
        assert_eq!(url("git@github.com:o/r.git"), expected);
        assert_eq!(url("ssh://git@github.com/o/r"), expected);
        assert_eq!(url("https://gitlab.com/o/r.git"), None);
        assert_eq!(url("git@example.com:o/r.git"), None);
    }

    #[test]
    fn create_pr_necesita_un_origin_de_github() {
        let (_temp, repo) = temp_repo("pr-origin");
        let err = create_pr(&repo, "Título", "Cuerpo", "main").unwrap_err();
        assert!(err.contains("GitHub"), "{err}");
    }
}
