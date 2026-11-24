//! Orquestador: le das una tarea, Claude Code la divide en subtareas y elige un ejecutor para cada una;
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

use crate::config::Executor;

/// Quién planifica: el modelo más capaz, porque dividir bien la tarea es lo difícil.
const PLANNER: &str = "claude-opus-5-5";

/// Los ejecutores de Claude, siempre disponibles: id, para qué sirve (va en las instrucciones).
const CLAUDE: [(&str, &str); 3] = [
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

/// Los ejecutores que puede elegir el plan: los de Claude y los `[[executor]]` del config
/// (uno con el mismo `id` que uno de Claude lo reemplaza).
pub(crate) fn catalog(extra: &[Executor]) -> Vec<Executor> {
    let mut all: Vec<Executor> = CLAUDE
        .iter()
        .map(|(id, use_for)| Executor {
            id: id.to_string(),
            use_for: use_for.to_string(),
            command: format!("claude --model {id} {{prompt}}"),
        })
        .collect();
    for e in extra {
        match all.iter_mut().find(|a| a.id == e.id) {
            Some(old) => *old = e.clone(),
            None => all.push(e.clone()),
        }
    }
    all
}

/// Del catálogo, solo los que tienen su programa (la primera palabra del comando) instalado.
fn installed(catalog: &[Executor]) -> Vec<Executor> {
    use crate::agents::{Located, locate};
    catalog
        .iter()
        .filter(|e| {
            let program = e.command.split_whitespace().next().unwrap_or_default();
            matches!(locate(program), Some(Located::Ready(_)))
        })
        .cloned()
        .collect()
}

/// Más que esto ya no se puede seguir en pestañas (ni conviene gastar tanto de una vez).
const MAX_SUBTASKS: usize = 6;

/// Cómo se llama un ejecutor en la ventana: los de Claude, por su modelo ("claude-haiku-5-5" → "haiku");
/// los demás, por su id.
pub(crate) fn short_name(id: &str) -> &str {
    match id.strip_prefix("claude-") {
        Some(rest) => rest.split('-').next().unwrap_or(id),
        None => id,
    }
}

#[derive(serde::Deserialize, Clone, PartialEq, Debug)]
pub(crate) struct Subtask {
    /// Corto: da nombre a la rama y la carpeta del worktree.
    pub(crate) title: String,
    /// Lo que recibe el agente: tiene que entenderse sin ver las otras subtareas.
    pub(crate) prompt: String,
    /// El `id` de un ejecutor del catálogo.
    pub(crate) executor: String,
    /// Índices (desde 0) de subtareas anteriores cuyo código necesita: arranca cuando terminan.
    #[serde(default)]
    pub(crate) after: Vec<usize>,
}

/// El esquema que `claude -p` hace cumplir a la respuesta.
fn schema(executors: &[Executor]) -> String {
    let models: Vec<String> = (executors.iter())
        .map(|e| serde_json::Value::from(e.id.as_str()).to_string())
        .collect();
    format!(
        r#"{{"type":"object","properties":{{"subtasks":{{"type":"array","minItems":1,"maxItems":{MAX_SUBTASKS},"items":{{"type":"object","properties":{{"title":{{"type":"string","maxLength":40}},"prompt":{{"type":"string"}},"executor":{{"type":"string","enum":[{}]}},"after":{{"type":"array","items":{{"type":"integer","minimum":0}}}}}},"required":["title","prompt","executor"]}}}}}},"required":["subtasks"]}}"#,
        models.join(",")
    )
}

/// Las instrucciones para el planificador, con la tarea al final.
fn instructions(task: &str, executors: &[Executor]) -> String {
    let models: Vec<String> = (executors.iter())
        .map(|e| format!("- {}: {}", e.id, e.use_for))
        .collect();
    format!(
        "Eres el planificador de Orion. Divide la tarea de abajo en subtareas que distintos agentes \
         hagan en paralelo, cada uno en su propia rama de git. Prefiere subtareas independientes. \
         Si la tarea no se puede dividir, devuelve una sola subtarea.\n\
         Puedes leer el repositorio para planificar mejor, pero no cambies nada.\n\
         Para cada subtarea: un título corto (unas 4 palabras), un prompt completo que se entienda \
         solo, y el ejecutor más barato que la haga bien, según para qué sirve cada uno:\n{}\n\
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
/// `catalog` es el catálogo completo: se ofrecen solo los ejecutores instalados.
pub(crate) fn plan(
    task: &str,
    dir: &Path,
    planner: &Planner,
    catalog: &[Executor],
) -> Result<Vec<Subtask>, String> {
    let executors = installed(catalog);
    let mut child = claude()?
        .current_dir(dir)
        .args(["-p", "--model", PLANNER, "--output-format", "json"])
        .args(["--json-schema", &schema(&executors)])
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
        .write_all(instructions(task, &executors).as_bytes())
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
    parse_plan(&stdout, &executors)
}

/// Lee lo que imprime `claude -p --output-format json`: el plan viene en `structured_output`.
fn parse_plan(stdout: &str, executors: &[Executor]) -> Result<Vec<Subtask>, String> {
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
    // El esquema ya lo exige, pero el comando que se lanza sale de acá: no se confía en lo que llegó.
    if let Some(bad) = (subtasks.iter()).find(|s| !executors.iter().any(|e| e.id == s.executor)) {
        return Err(format!(
            "el plan pidió un ejecutor desconocido: {}",
            bad.executor
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

use crate::agents::git;

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

/// Lo que se escribe en la pestaña del subagente: la plantilla de su ejecutor con el prompt puesto.
/// El prompt va en una línea (un salto de línea en la shell sería un Enter), sin comillas dobles
/// (cmd no las admite dentro de un argumento, y Windows PowerShell las pierde al pasarlas) y sin `!` ni `%`
/// (los shims .bat/.cmd pasan por cmd, que los expande). Sin `{prompt}` en la plantilla, va al final.
pub(crate) fn agent_command(
    shell: &crate::pty::ShellKind,
    subtask: &Subtask,
    catalog: &[Executor],
) -> String {
    let prompt: String = subtask
        .prompt
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "'")
        .replace(['!', '%'], "");
    let quoted = shell.quote_path(&prompt);
    let template = (catalog.iter())
        .find(|e| e.id == subtask.executor)
        .map_or_else(
            || format!("claude --model {} {{prompt}}", subtask.executor),
            |e| e.command.clone(),
        );
    if template.contains("{prompt}") {
        template.replace("{prompt}", &quoted)
    } else {
        format!("{template} {quoted}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::SHELLS;

    fn subtask(model: &str) -> String {
        format!(r#"{{"title":"Tests","prompt":"Escribe tests","executor":"{model}"}}"#)
    }

    /// Un plan con una subtarea por cada `after` (en JSON).
    fn with_after(after: &[&str]) -> String {
        let items: Vec<String> = after
            .iter()
            .map(|a| {
                format!(r#"{{"title":"T","prompt":"p","executor":"claude-haiku-5-5","after":{a}}}"#)
            })
            .collect();
        format!(
            r#"{{"structured_output":{{"subtasks":[{}]}}}}"#,
            items.join(",")
        )
    }

    #[test]
    fn after_only_points_back() {
        let plan = parse_plan(&with_after(&["[]", "[0]", "[0,1]"]), &catalog(&[])).unwrap();
        assert!(plan[0].after.is_empty());
        assert_eq!(plan[2].after, vec![0, 1]);
        // Sin `after`, independiente.
        let out = format!(
            r#"{{"structured_output":{{"subtasks":[{}]}}}}"#,
            subtask("claude-haiku-5-5")
        );
        assert!(parse_plan(&out, &catalog(&[])).unwrap()[0].after.is_empty());
        // A sí misma o hacia adelante: error claro.
        let own = parse_plan(&with_after(&["[]", "[1]"]), &catalog(&[])).unwrap_err();
        assert!(own.contains("anteriores"), "{own}");
        assert!(parse_plan(&with_after(&["[1]", "[]"]), &catalog(&[])).is_err());
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
        let plan = parse_plan(&out, &catalog(&[])).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].title, "Tests");
        assert_eq!(short_name(&plan[0].executor), "haiku");
    }

    #[test]
    fn a_bad_plan_is_an_error_not_a_launch() {
        let unknown = format!(
            r#"{{"structured_output":{{"subtasks":[{}]}}}}"#,
            subtask("gpt-9")
        );
        assert!(
            parse_plan(&unknown, &catalog(&[]))
                .unwrap_err()
                .contains("gpt-9")
        );
        assert!(parse_plan(r#"{"structured_output":{"subtasks":[]}}"#, &catalog(&[])).is_err());
        let failed = r#"{"is_error":true,"result":"Not logged in"}"#;
        assert!(
            parse_plan(failed, &catalog(&[]))
                .unwrap_err()
                .contains("Not logged in")
        );
        assert!(parse_plan("no es json", &catalog(&[])).is_err());
    }

    #[test]
    fn the_schema_is_valid_json_and_lists_every_executor() {
        let all = catalog(&[executor("junie", "mix", "junie --prompt={prompt}")]);
        let schema: serde_json::Value = serde_json::from_str(&schema(&all)).unwrap();
        let ids = &schema["properties"]["subtasks"]["items"]["properties"]["executor"]["enum"];
        assert_eq!(ids.as_array().unwrap().len(), 4);
        assert!(instructions("x", &all).contains("- junie: mix"));
    }

    fn executor(id: &str, use_for: &str, command: &str) -> Executor {
        Executor {
            id: id.into(),
            use_for: use_for.into(),
            command: command.into(),
        }
    }

    #[test]
    fn the_catalog_has_claude_and_the_configured_ones() {
        let ids = |c: &[Executor]| c.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        let base = catalog(&[]);
        assert_eq!(
            ids(&base),
            ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]
        );
        assert_eq!(base[1].command, "claude --model claude-sonnet-5-5 {prompt}");
        // Uno nuevo se agrega; uno con el id de uno de Claude lo reemplaza.
        let own = executor("claude-haiku-5-5", "otro", "mi-haiku {prompt}");
        let plus = catalog(&[executor("opencode", "barato", "opencode"), own.clone()]);
        assert_eq!(plus.len(), 4);
        assert_eq!(plus[2], own);
        assert_eq!(plus[3].id, "opencode");
    }

    #[test]
    fn a_plan_with_an_executor_outside_the_catalog_is_rejected() {
        let out = format!(
            r#"{{"structured_output":{{"subtasks":[{}]}}}}"#,
            subtask("junie")
        );
        assert!(
            parse_plan(&out, &catalog(&[]))
                .unwrap_err()
                .contains("junie")
        );
        let with = catalog(&[executor("junie", "x", "junie")]);
        assert!(parse_plan(&out, &with).is_ok());
    }

    #[test]
    fn the_template_gets_the_prompt_without_bang_or_percent() {
        let s = |executor: &str| Subtask {
            title: "x".into(),
            prompt: "Corre 100% de los tests! ya".into(),
            executor: executor.into(),
            after: Vec::new(),
        };
        let all = catalog(&[
            executor("junie", "", "junie --model=mix.normal --prompt={prompt}"),
            executor("opencode", "", "opencode -m deepseek/x --prompt {prompt}"),
            executor("plano", "", "plano --rapido"),
        ]);
        let q = "'";
        let sh = &SHELLS[0];
        let junie = agent_command(sh, &s("junie"), &all);
        assert_eq!(
            junie,
            format!("junie --model=mix.normal --prompt={q}Corre 100 de los tests ya{q}")
        );
        let opencode = agent_command(sh, &s("opencode"), &all);
        assert_eq!(
            opencode,
            format!("opencode -m deepseek/x --prompt {q}Corre 100 de los tests ya{q}")
        );
        // Sin {prompt}, va al final.
        let plano = agent_command(sh, &s("plano"), &all);
        assert_eq!(
            plano,
            format!("plano --rapido {q}Corre 100 de los tests ya{q}")
        );
        for c in [junie, opencode, plano] {
            assert!(!c.contains(['!', '%']), "{c}");
        }
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
            executor: "claude-sonnet-5-5".into(),
            after: Vec::new(),
        };
        let command = agent_command(&SHELLS[0], &s, &catalog(&[]));
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
        &catalog(&[]),
    )
    .unwrap();
    assert!(!plan.is_empty());
    eprintln!("{plan:#?}");
}
