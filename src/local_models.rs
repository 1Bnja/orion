//! Modelos de Ollama y plantillas para ejecutarlos con agentes instalados.

use crate::config::Executor;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Nombres de `ollama list`; sin Ollama o si falla, vacío. Cachea también los fallos por 30 s.
/// La primera consulta y cada renovación esperan al proceso; no comprueba capacidades del modelo.
/// Respeta el servidor configurado en Ollama: listar no garantiza que la inferencia sea local.
pub(crate) fn ollama_models() -> Vec<String> {
    static CACHE: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match &*cache {
        Some((at, models)) if at.elapsed() < Duration::from_secs(30) => models.clone(),
        _ => {
            let models = crate::update::quiet("ollama")
                .arg("list")
                .output()
                .ok()
                .filter(|out| out.status.success())
                .map(|out| parse_ollama_list(&String::from_utf8_lossy(&out.stdout)))
                .unwrap_or_default();
            *cache = Some((Instant::now(), models.clone()));
            models
        }
    }
}

/// Primera columna de la tabla de Ollama, saltando la cabecera y las líneas vacías.
pub(crate) fn parse_ollama_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .skip(1)
        .map(str::to_owned)
        .collect()
}

/// Plantillas interactivas; el prompt lo cita el orquestador al reemplazar `{prompt}`.
/// OpenCode necesita el provider `ollama` y el modelo configurados, con Ollama en ejecución.
/// Flags y configuración: https://opencode.ai/v1/docs/cli/#tui y
/// https://opencode.ai/v1/docs/providers/#ollama.
/// Aider se omite aunque esté instalado: `--message` y `--message-file` desactivan el chat.
/// `--load` permite iniciar en modo interactivo, pero exige un archivo de /comandos;
/// esta API solo devuelve plantillas y no crea ese archivo. `have_aider` queda reservado.
/// Véase https://aider.chat/docs/config/options.html y https://aider.chat/docs/llms/ollama.html
/// (`--model ollama_chat/<modelo>`). No pasar `{prompt}` como archivo ni por una tubería.
/// «Sin red» supone modelo descargado y servidor local; no configura ni audita los agentes.
pub(crate) fn executors_for(model: &str, have_opencode: bool, _have_aider: bool) -> Vec<Executor> {
    // El nombre entra en una shell: solo caracteres de nombres de modelo, sin metacaracteres.
    if !have_opencode
        || model.is_empty()
        || !model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._/:".contains(c))
    {
        return Vec::new();
    }
    let slug = model
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .to_ascii_lowercase();
    if slug.is_empty() {
        return Vec::new();
    }
    vec![Executor {
        id: format!("ollama-{slug}"),
        use_for: "local, gratis, sin red; tareas simples (con modelo descargado y servidor local)"
            .into(),
        command: format!("opencode -m ollama/{model} --prompt {{prompt}}"),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_without_the_header_or_other_columns() {
        let stdout = "NAME ID SIZE MODIFIED\n\
                      deepseek-r1:14b aabbcc 9.0GB 2 hours ago\n\
                      qwen2.5-coder:7b ddeeff 4.7GB 1 day ago\n";
        assert_eq!(
            parse_ollama_list(stdout),
            ["deepseek-r1:14b", "qwen2.5-coder:7b"]
        );
    }

    #[test]
    fn skips_blank_lines_even_before_the_header() {
        assert_eq!(
            parse_ollama_list("\n \r\nNAME ID SIZE MODIFIED\r\n\r\ndeepseek-r1:14b abc\n \n"),
            ["deepseek-r1:14b"]
        );
    }

    #[test]
    fn accepts_multiple_spaces_and_tabs() {
        assert_eq!(
            parse_ollama_list("  NAME    ID\n  qwen2.5-coder:7b   abc\n\tteam/model:latest\tabc"),
            ["qwen2.5-coder:7b", "team/model:latest"]
        );
    }

    #[test]
    fn empty_output_and_header_only_have_no_models() {
        for stdout in ["", " \n\t\r\n", "NAME ID SIZE MODIFIED\n"] {
            assert!(parse_ollama_list(stdout).is_empty());
        }
    }

    #[test]
    fn ids_are_model_slugs_and_keep_the_tag() {
        for (model, id) in [
            ("deepseek-r1:14b", "ollama-deepseek-r1-14b"),
            ("qwen2.5-coder:7b", "ollama-qwen2-5-coder-7b"),
            ("Team/Model_Name:latest", "ollama-team-model-name-latest"),
        ] {
            assert_eq!(executors_for(model, true, false)[0].id, id);
        }
        assert_ne!(
            executors_for("deepseek-r1:7b", true, false)[0].id,
            executors_for("deepseek-r1:14b", true, false)[0].id
        );
    }

    #[test]
    fn only_offers_agents_that_accept_an_interactive_initial_prompt() {
        for have_aider in [false, true] {
            assert!(executors_for("deepseek-r1:14b", false, have_aider).is_empty());
            let executors = executors_for("deepseek-r1:14b", true, have_aider);
            assert_eq!(executors.len(), 1);
            for executor in executors {
                assert!(executor.command.contains("{prompt}"));
                assert_eq!(
                    executor.command,
                    "opencode -m ollama/deepseek-r1:14b --prompt {prompt}"
                );
            }
        }
    }

    #[test]
    fn rejects_empty_names_and_shell_metacharacters() {
        for model in [
            "",
            ":/",
            "a b",
            "a;whoami",
            "a$(whoami)",
            "a`whoami`",
            "a\n",
            "a{prompt}",
        ] {
            assert!(executors_for(model, true, true).is_empty(), "{model:?}");
        }
    }
}
