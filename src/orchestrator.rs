//! Orquestador: le das una tarea, Claude Code la divide en subtareas y elige un modelo para cada una;
//! después Orion abre una pestaña de agente por subtarea, cada una en su propio git worktree.
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
}

/// El esquema que `claude -p` hace cumplir a la respuesta.
fn schema() -> String {
    let models: Vec<String> = MODELS.iter().map(|(id, _)| format!("\"{id}\"")).collect();
    format!(
        r#"{{"type":"object","properties":{{"subtasks":{{"type":"array","minItems":1,"maxItems":{MAX_SUBTASKS},"items":{{"type":"object","properties":{{"title":{{"type":"string","maxLength":40}},"prompt":{{"type":"string"}},"model":{{"type":"string","enum":[{}]}}}},"required":["title","prompt","model"]}}}}}},"required":["subtasks"]}}"#,
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
         puedan hacer en paralelo, cada uno en su propia rama de git, sin depender del trabajo de \
         los otros. Si la tarea no se puede dividir así, devuelve una sola subtarea.\n\
         Puedes leer el repositorio para planificar mejor, pero no cambies nada.\n\
         Para cada subtarea: un título corto (unas 4 palabras), un prompt completo que se entienda \
         solo, y el modelo más barato que la haga bien:\n{}\n\
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
    Ok(subtasks)
}

/// Dónde buscar `claude`, además del PATH.
/// macOS: una app abierta desde el Finder no hereda el PATH de tu terminal, así que se miran también
/// las carpetas donde lo dejan sus instaladores (el nativo, Homebrew y npm).
/// ponytail: lista fija; si alguien lo instala en otro lado, agregar `claude_path` a config.toml.
fn claude_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".claude/local"));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    dirs
}

/// Windows: el instalador nativo deja claude.exe; npm, claude.cmd (que `Command::new("claude")` no encuentra).
#[cfg(windows)]
const CLAUDE_NAMES: [&str; 2] = ["claude.exe", "claude.cmd"];
#[cfg(not(windows))]
const CLAUDE_NAMES: [&str; 1] = ["claude"];

/// El primer `names` que exista en `dirs`, en orden: primero todas las carpetas con el primer nombre.
fn find_in(dirs: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .flat_map(|name| dirs.iter().map(move |d| d.join(name)))
        .find(|p| p.is_file())
}

fn claude() -> Result<Command, String> {
    let path = find_in(&claude_dirs(), &CLAUDE_NAMES)
        .ok_or("no encuentro Claude Code (`claude`): instálalo desde code.claude.com")?;
    Ok(crate::update::quiet(&path.to_string_lossy()))
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
    fn claude_is_found_in_the_first_folder_that_has_it() {
        let base = std::env::temp_dir().join(format!("orion-claude-{}", std::process::id()));
        let (a, b) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("claude.cmd"), "").unwrap();
        let dirs = [a.clone(), b.clone()];
        assert_eq!(
            find_in(&dirs, &["claude.exe", "claude.cmd"]),
            Some(b.join("claude.cmd"))
        );
        // El primer nombre gana aunque esté en una carpeta posterior.
        std::fs::write(b.join("claude.exe"), "").unwrap();
        assert_eq!(
            find_in(&dirs, &["claude.exe", "claude.cmd"]),
            Some(b.join("claude.exe"))
        );
        assert_eq!(find_in(&[a], &["claude"]), None);
        let _ = std::fs::remove_dir_all(&base);
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
