//! Orquestador: le das una tarea, Claude Code la divide en subtareas y elige un modelo para cada una;
//! después Orion abre una pestaña de agente por subtarea, cada una en su propio git worktree.
//! Si una subtarea necesita el código de otra (`after`), espera a que esa termine con commit y
//! arranca desde su rama: el plan avanza por rondas.
//!
//! Planifica `claude -p` (Claude Code sin ventana, con tu misma sesión: sin API key ni librerías de red),
//! con un esquema JSON para que la respuesta venga validada. Solo puede leer el repo: no edita nada.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// Quién planifica: el modelo más capaz, porque dividir bien la tarea es lo difícil.
const PLANNER: &str = "claude-opus-5-5";

/// Los modelos que puede elegir el plan, y para qué sirve cada uno (va en las instrucciones).
pub(crate) const MODELS: [(&str, &str); 3] = [
    (
        "claude-opus-5-5",
        "diseño, arquitectura o razonamiento difícil",
    ),
    (
        "claude-sonnet-5-5",
        "programar algo normal: una función, un arreglo, un test con lógica",
    ),
    (
        "claude-haiku-5-5",
        "tareas mecánicas: renombrar, formatear, documentar, cambios repetitivos",
    ),
];

/// Más que esto ya no se puede seguir en pestañas (ni conviene gastar tanto de una vez).
const MAX_SUBTASKS: usize = 6;

/// El nombre corto de un modelo, para la ventana: "claude-haiku-5-5" → "haiku".
pub(crate) fn short_name(model: &str) -> &str {
    model.split('-').nth(1).unwrap_or(model)
}

#[derive(serde::Deserialize, Clone, PartialEq, Debug)]
pub(crate) struct Subtask {
    /// Corto: da nombre a la rama y la carpeta del worktree.
    pub(crate) title: String,
    /// Lo que recibe el agente: tiene que entenderse sin ver las otras subtareas.
    pub(crate) prompt: String,
    pub(crate) model: String,
    /// Índices (desde 0) de subtareas anteriores cuyo código necesita: arranca cuando terminan.
    #[serde(default)]
    pub(crate) after: Vec<usize>,
}

/// El esquema que `claude -p` hace cumplir a la respuesta.
fn schema() -> String {
    let models: Vec<String> = MODELS.iter().map(|(id, _)| format!("\"{id}\"")).collect();
    format!(
        r#"{{"type":"object","properties":{{"subtasks":{{"type":"array","minItems":1,"maxItems":{MAX_SUBTASKS},"items":{{"type":"object","properties":{{"title":{{"type":"string","maxLength":40}},"prompt":{{"type":"string"}},"model":{{"type":"string","enum":[{}]}},"after":{{"type":"array","items":{{"type":"integer","minimum":0}}}}}},"required":["title","prompt","model"]}}}}}},"required":["subtasks"]}}"#,
        models.join(",")
    )
}

/// Las instrucciones para el planificador, con la tarea al final.
fn instructions(task: &str) -> String {
    let models: Vec<String> = MODELS
        .iter()
        .map(|(id, use_for)| format!("- {id}: {use_for}"))
        .collect();
    format!(
        "Eres el planificador de Orion. Divide la tarea de abajo en subtareas que distintos agentes \
         hagan en paralelo, cada uno en su propia rama de git. Prefiere subtareas independientes. \
         Si la tarea no se puede dividir, devuelve una sola subtarea.\n\
         Puedes leer el repositorio para planificar mejor, pero no cambies nada.\n\
         Para cada subtarea: un título corto (unas 4 palabras), un prompt completo que se entienda \
         solo, y el modelo más barato que la haga bien:\n{}\n\
         `after` (opcional): los índices, desde 0, de subtareas ANTERIORES cuyo código necesita de \
         verdad (por ejemplo, usa una función que otra crea). Esa subtarea arranca cuando aquellas \
         terminan, desde sus ramas. Úsalo solo cuando haga falta: cada `after` hace esperar.\n\
         Cada prompt tiene que pedirle al agente que, al terminar, haga commit de su trabajo en su rama.\n\
         Si conviene, la última subtarea puede ser una revisión del conjunto que dependa de las demás.\n\
         Escribe títulos y prompts en el idioma de la tarea.\n\nTarea:\n{task}",
        models.join("\n")
    )
}

/// El `claude -p` que está planificando, para poder cortarlo al cancelar (si no, sigue gastando uso).
pub(crate) type Planner = Arc<Mutex<Option<Child>>>;

/// Corta la planificación en curso, si hay una.
pub(crate) fn cancel(planner: &Planner) {
    if let Some(mut child) = planner.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Pide el plan para `task`, trabajando en `dir`. Tarda (segundos o minutos): va en un hilo aparte.
/// Mientras corre, el proceso queda en `planner` para que `cancel` pueda cortarlo.
pub(crate) fn plan(task: &str, dir: &Path, planner: &Planner) -> Result<Vec<Subtask>, String> {
    let mut child = claude()?
        .current_dir(dir)
        .args(["-p", "--model", PLANNER, "--output-format", "json"])
        .args(["--json-schema", &schema()])
        // Solo leer. Y sin MCP ni skills: cargarlos multiplica el contexto (y lo que cuesta) sin ayudar a planificar.
        .args(["--tools", "Read,Glob,Grep", "--strict-mcp-config"])
        .args(["--disable-slash-commands", "--no-session-persistence"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("no pude abrir Claude Code ({e})"))?;
    // La tarea va por stdin: en Windows, claude.cmd tiene reglas de comillas delicadas para los argumentos.
    let mut stdin = child.stdin.take().expect("stdin es piped");
    stdin
        .write_all(instructions(task).as_bytes())
        .map_err(|e| e.to_string())?;
    drop(stdin); // cerrarlo: así claude sabe que el prompt terminó
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    *planner.lock().unwrap() = Some(child);
    // Leer hasta que termine (o lo maten). stderr en su hilo: si se llenara mientras esperamos
    // stdout, claude quedaría bloqueado escribiendo y nunca terminaría.
    let stderr = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = err.read_to_string(&mut text);
        text
    });
    let mut stdout = String::new();
    let _ = out.read_to_string(&mut stdout);
    let stderr = stderr.join().unwrap_or_default();
    let Some(mut child) = planner.lock().unwrap().take() else {
        return Err("cancelado".into()); // `cancel` se lo llevó
    };
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() && stdout.trim().is_empty() {
        return Err(format!("Claude Code falló: {}", stderr.trim()));
    }
    parse_plan(&stdout)
}

/// Lee lo que imprime `claude -p --output-format json`: el plan viene en `structured_output`.
fn parse_plan(stdout: &str) -> Result<Vec<Subtask>, String> {
    #[derive(serde::Deserialize)]
    struct Output {
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        result: Option<String>,
        structured_output: Option<Plan>,
    }
    #[derive(serde::Deserialize)]
    struct Plan {
        subtasks: Vec<Subtask>,
    }
    let output: Output = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("no entendí la respuesta de Claude Code ({e})"))?;
    if output.is_error {
        let why = output.result.unwrap_or_default();
        return Err(format!("Claude Code no pudo planificar: {why}"));
    }
    let plan = output
        .structured_output
        .ok_or("Claude Code no devolvió un plan")?;
    let subtasks: Vec<Subtask> = plan.subtasks.into_iter().take(MAX_SUBTASKS).collect();
    if subtasks.is_empty() {
        return Err("el plan vino vacío".into());
    }
    // El esquema ya lo exige, pero el modelo que se lanza sale de acá: no se confía en lo que llegó.
    if let Some(bad) = subtasks
        .iter()
        .find(|s| !MODELS.iter().any(|(id, _)| *id == s.model))
    {
        return Err(format!(
            "el plan pidió un modelo desconocido: {}",
            bad.model
        ));
    }
    // Solo hacia atrás: así no puede haber ciclos y cada ronda sabe de qué ramas parte.
    for (i, s) in subtasks.iter().enumerate() {
        if let Some(bad) = s.after.iter().find(|&&j| j >= i) {
            return Err(format!(
                "el plan dice que «{}» (subtarea {i}) va después de la {bad}: solo puede depender de subtareas anteriores",
                s.title
            ));
        }
    }
    Ok(subtasks)
}

/// Cómo está una dependencia, para decidir si la subtarea que la espera puede arrancar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Dep {
    /// Todavía no arranca, o su agente sigue abierto.
    Pending,
    /// Su agente terminó (el usuario lo cerró), con su código y si dejó commits en su rama.
    Done { code: Option<i32>, commits: bool },
    /// Se cerró su pestaña antes de que el agente terminara.
    Closed,
    /// No pudo arrancar (y lo que dependa de ella tampoco).
    Blocked,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Gate {
    Ready,
    Wait,
    Blocked(String),
}

/// Si una subtarea puede arrancar, dados `(título, estado)` de sus dependencias.
/// Basta una que falle para bloquearla, aunque otras sigan trabajando: así se avisa apenas se sabe.
pub(crate) fn gate(deps: &[(&str, Dep)]) -> Gate {
    let mut wait = false;
    for &(title, dep) in deps {
        let why = match dep {
            Dep::Pending => {
                wait = true;
                continue;
            }
            Dep::Done {
                code: Some(0) | None,
                commits: true,
            } => continue,
            Dep::Done {
                code: Some(0) | None,
                commits: false,
            } => format!("«{title}» terminó sin hacer commit en su rama"),
            Dep::Done { code: Some(c), .. } => format!("«{title}» terminó con error (código {c})"),
            Dep::Closed => format!("se cerró la pestaña de «{title}» antes de que terminara"),
            Dep::Blocked => format!("«{title}» quedó bloqueada"),
        };
        return Gate::Blocked(why);
    }
    if wait { Gate::Wait } else { Gate::Ready }
}

/// Corre git en `dir` y devuelve lo que imprimió, o su mensaje de error.
/// ponytail: copia del `git` privado de agents.rs (ese archivo no se tocó en este cambio); unificarlas.
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

/// Un worktree para la subtarea `title` con el código de las ramas de sus dependencias: parte de la
/// primera y fusiona las demás. Si una fusión choca, la deshace y devuelve el error (el worktree queda
/// limpio, por si quieres resolverlo a mano). Devuelve la carpeta, su rama y el commit del que parte
/// el agente (para saber después si hizo commits propios).
pub(crate) fn prepare_worktree(
    repo: &Path,
    title: &str,
    deps: &[String],
) -> Result<(PathBuf, String, String), String> {
    let path = crate::agents::create_worktree(repo, title, deps.first().map(String::as_str))?;
    for branch in deps.iter().skip(1) {
        if let Err(e) = git(&path, &["merge", "--no-edit", branch]) {
            let _ = git(&path, &["merge", "--abort"]);
            return Err(format!(
                "chocó al fusionar {branch} en {}: deshice la fusión ({e})",
                path.display()
            ));
        }
    }
    let branch = git(&path, &["branch", "--show-current"])?;
    let base = git(&path, &["rev-parse", "HEAD"])?;
    Ok((path, branch, base))
}

/// Si `branch` tiene commits que no están en `base`. Ante un error de git, no.
pub(crate) fn has_commits(repo: &Path, base: &str, branch: &str) -> bool {
    git(repo, &["rev-list", "--count", &format!("{base}..{branch}")])
        .is_ok_and(|n| n.parse::<u32>().is_ok_and(|n| n > 0))
}

fn claude() -> Result<Command, String> {
    use crate::agents::{Located, locate};
    match locate("claude") {
        Some(Located::Ready(path)) => Ok(crate::update::quiet(&path.to_string_lossy())),
        Some(Located::Broken { missing }) => Err(format!(
            "la instalación de Claude Code está rota (falta {}): reinstálala con `npm install -g @anthropic-ai/claude-code`",
            missing.display()
        )),
        None => Err("no encuentro Claude Code (`claude`): instálalo desde code.claude.com".into()),
    }
}

/// Lo que se escribe en la pestaña del subagente: Claude Code interactivo, con su modelo y su prompt.
/// El prompt va en una línea (un salto de línea en la shell sería un Enter) y sin comillas dobles
/// (cmd no las admite dentro de un argumento, y Windows PowerShell las pierde al pasarlas).
pub(crate) fn agent_command(shell: &crate::pty::ShellKind, subtask: &Subtask) -> String {
    let prompt: String = subtask
        .prompt
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "'");
    let quoted = shell.quote_path(&prompt);
    format!("claude --model {} {quoted}", subtask.model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::SHELLS;

    fn subtask(model: &str) -> String {
        format!(r#"{{"title":"Tests","prompt":"Escribe tests","model":"{model}"}}"#)
    }

    /// Un plan con una subtarea por cada `after` (en JSON).
    fn with_after(after: &[&str]) -> String {
        let items: Vec<String> = after
            .iter()
            .map(|a| {
                format!(r#"{{"title":"T","prompt":"p","model":"claude-haiku-5-5","after":{a}}}"#)
            })
            .collect();
        format!(
            r#"{{"structured_output":{{"subtasks":[{}]}}}}"#,
            items.join(",")
        )
    }

    #[test]
    fn after_only_points_back() {
        let plan = parse_plan(&with_after(&["[]", "[0]", "[0,1]"])).unwrap();
        assert!(plan[0].after.is_empty());
        assert_eq!(plan[2].after, vec![0, 1]);
        // Sin `after`, independiente.
        let out = format!(
            r#"{{"structured_output":{{"subtasks":[{}]}}}}"#,
            subtask("claude-haiku-5-5")
        );
        assert!(parse_plan(&out).unwrap()[0].after.is_empty());
        // A sí misma o hacia adelante: error claro.
        let own = parse_plan(&with_after(&["[]", "[1]"])).unwrap_err();
        assert!(own.contains("anteriores"), "{own}");
        assert!(parse_plan(&with_after(&["[1]", "[]"])).is_err());
    }

    #[test]
    fn the_gate_waits_launches_or_blocks() {
        let done = |code, commits| Dep::Done { code, commits };
        assert_eq!(gate(&[]), Gate::Ready);
        let ok = [("a", done(Some(0), true)), ("b", done(None, true))];
        assert_eq!(gate(&ok), Gate::Ready);
        assert_eq!(gate(&[ok[0], ("b", Dep::Pending)]), Gate::Wait);
        let blocked = |deps: &[(&str, Dep)]| match gate(deps) {
            Gate::Blocked(why) => why,
            other => panic!("{other:?}"),
        };
        assert!(blocked(&[("a", done(Some(0), false))]).contains("sin hacer commit"));
        assert!(blocked(&[("a", done(Some(2), true))]).contains("código 2"));
        // Una que falla bloquea aunque otra siga trabajando.
        assert!(blocked(&[("a", Dep::Pending), ("b", Dep::Closed)]).contains("«b»"));
        assert!(blocked(&[("a", Dep::Blocked)]).contains("bloqueada"));
    }

    #[test]
    fn dependent_worktrees_start_from_their_dependencies() {
        let base = std::env::temp_dir().join(format!("orion-rondas-{}", std::process::id()));
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |dir: &Path, args: &[&str]| git(dir, args).unwrap();
        run(&repo, &["init", "-q"]);
        run(&repo, &["config", "user.name", "t"]);
        run(&repo, &["config", "user.email", "t@t"]);
        std::fs::write(repo.join("comun.txt"), "base\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "base"]);
        // Un "agente" que escribe un archivo y hace commit en su rama.
        let work = |title: &str, deps: &[String], file: &str, text: &str| {
            let (path, branch, start) = prepare_worktree(&repo, title, deps).unwrap();
            assert!(!has_commits(&repo, &start, &branch), "todavía no hizo nada");
            std::fs::write(path.join(file), text).unwrap();
            run(&path, &["add", "."]);
            run(&path, &["commit", "-q", "-m", title]);
            assert!(has_commits(&repo, &start, &branch));
            branch
        };
        let a = work("A", &[], "a.txt", "a");
        let b = work("B", &[], "b.txt", "b");
        // Depende de una: parte de su rama y tiene su commit.
        let after_a = work("D", std::slice::from_ref(&a), "d.txt", "d");
        assert!(run(&repo, &["log", "--format=%s", &after_a]).contains('A'));
        // Depende de las dos: parte de A y fusiona B.
        let (both, _, _) = prepare_worktree(&repo, "C", &[a, b]).unwrap();
        assert!(both.join("a.txt").exists() && both.join("b.txt").exists());

        // Dos ramas que cambian la misma línea: la fusión choca y no queda a medias.
        let x = work("X", &[], "comun.txt", "x\n");
        let y = work("Y", &[], "comun.txt", "y\n");
        let err = prepare_worktree(&repo, "Z", &[x, y]).unwrap_err();
        assert!(err.contains("chocó"), "{err}");
        let z = repo.join(".orion/worktrees/z-1");
        assert_eq!(run(&z, &["status", "--porcelain"]), "");
        assert!(git(&z, &["rev-parse", "--verify", "--quiet", "MERGE_HEAD"]).is_err());
        let text = std::fs::read_to_string(z.join("comun.txt")).unwrap();
        assert_eq!(text.trim(), "x");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_plan_comes_in_structured_output() {
        let out = format!(
            r#"{{"type":"result","is_error":false,"result":"…","structured_output":{{"subtasks":[{}]}}}}"#,
            subtask("claude-haiku-5-5")
        );
        let plan = parse_plan(&out).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].title, "Tests");
        assert_eq!(short_name(&plan[0].model), "haiku");
    }

    #[test]
    fn a_bad_plan_is_an_error_not_a_launch() {
        let unknown = format!(
            r#"{{"structured_output":{{"subtasks":[{}]}}}}"#,
            subtask("gpt-9")
        );
        assert!(parse_plan(&unknown).unwrap_err().contains("gpt-9"));
        assert!(parse_plan(r#"{"structured_output":{"subtasks":[]}}"#).is_err());
        let failed = r#"{"is_error":true,"result":"Not logged in"}"#;
        assert!(parse_plan(failed).unwrap_err().contains("Not logged in"));
        assert!(parse_plan("no es json").is_err());
    }

    #[test]
    fn the_schema_is_valid_json_and_lists_every_model() {
        let schema: serde_json::Value = serde_json::from_str(&schema()).unwrap();
        let models = &schema["properties"]["subtasks"]["items"]["properties"]["model"]["enum"];
        assert_eq!(models.as_array().unwrap().len(), MODELS.len());
    }

    #[test]
    fn cancel_stops_the_running_planner() {
        let long = if cfg!(windows) {
            Command::new("ping")
                .args(["-n", "30", "127.0.0.1"])
                .stdout(Stdio::null())
                .spawn()
        } else {
            Command::new("sleep").arg("30").spawn()
        };
        let planner = Planner::default();
        *planner.lock().unwrap() = Some(long.unwrap());
        let start = std::time::Instant::now();
        cancel(&planner);
        assert!(planner.lock().unwrap().is_none());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "no se cortó"
        );
        cancel(&planner); // sin nada corriendo, no hace nada
    }

    #[test]
    fn the_prompt_goes_in_one_quoted_line() {
        let s = Subtask {
            title: "x".into(),
            prompt: "Arregla \"el\" test\nde l'app".into(),
            model: "claude-sonnet-5-5".into(),
            after: Vec::new(),
        };
        let command = agent_command(&SHELLS[0], &s);
        // PowerShell (Windows) y zsh (Mac) usan comillas simples, cada una con su escape.
        let expected = if cfg!(windows) {
            "claude --model claude-sonnet-5-5 'Arregla ''el'' test de l''app'"
        } else {
            r"claude --model claude-sonnet-5-5 'Arregla '\''el'\'' test de l'\''app'"
        };
        assert_eq!(command, expected);
    }
}

/// Llama de verdad a Claude Code (gasta uso de tu cuenta): `cargo test real_plan -- --ignored`.
#[cfg(test)]
#[test]
#[ignore]
fn real_plan() {
    let dir = std::env::current_dir().unwrap();
    let plan = plan(
        "Agrega un comentario de una línea arriba de fn main en src/main.rs",
        &dir,
        &Planner::default(),
    )
    .unwrap();
    assert!(!plan.is_empty());
    eprintln!("{plan:#?}");
}
