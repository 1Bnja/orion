//! La sección «Agentes» de Configuración: qué CLIs hay, cuáles usa el orquestador, los modelos de
//! Ollama y un formulario para sumar un agente sin editar config.toml a mano.

use super::Orion;
use super::chrome::medium;
use super::settings::{setting_row, viewport_id};
use crate::agents::{Located, PROVIDERS, locate, slug};
use crate::colors::theme;
use crate::config::{Agent, Executor, Settings};
use crate::orchestrator::{ExecutorSource, Subtask, agent_command, catalog_with_sources};
use crate::pty::ShellKind;
use eframe::egui;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Lo que recibe el agente al probarlo: corto, y sin nada que una shell pueda interpretar.
const TEST_PROMPT: &str = "Responde solo con OK";

/// Cada cuánto se vuelve a preguntar a Ollama (lo mismo que dura su caché).
const MODELS_REFRESH: Duration = Duration::from_secs(30);

/// Estado de la sección: se conserva mientras Orion siga abierto, así no se pierde lo escrito.
#[derive(Default)]
pub(super) struct AgentsPanel {
    form: Form,
    /// Los llena un hilo aparte: `ollama list` puede tardar y trabaría la ventana.
    models: Arc<Mutex<Vec<String>>>,
    models_asked: Option<Instant>,
    /// El último programa buscado y qué se encontró, para no recorrer el PATH en cada frame.
    located: Option<(String, Instant, Option<Located>)>,
    /// El nombre del último agente guardado, para confirmarlo.
    saved: Option<String>,
    /// Pulsaste «Probar»: lo recoge `settings_window`, que puede abrir pestañas.
    pub(super) test: Option<TestRun>,
}

/// Lo que se va a lanzar al probar un agente.
pub(super) struct TestRun {
    name: String,
    command: String,
    /// Es la plantilla del orquestador: el prompt se pone igual que en una subtarea.
    template: bool,
}

/// El formulario «Agregar agente».
#[derive(Default, Clone, PartialEq)]
struct Form {
    /// Índice en `PROVIDERS` de la plantilla elegida; None = personalizado.
    from: Option<usize>,
    name: String,
    command: String,
    worktree: bool,
    orchestrate: bool,
    use_for: String,
    template: String,
}

impl Form {
    /// El formulario relleno con un provider, o vacío.
    fn from_provider(from: Option<usize>) -> Self {
        let Some(provider) = from.and_then(|i| PROVIDERS.get(i)) else {
            return Self::default();
        };
        Self {
            from,
            name: provider.name.into(),
            command: provider.command.into(),
            worktree: true,
            orchestrate: provider.executor.is_some(),
            use_for: provider.use_for.into(),
            template: provider.executor.unwrap_or_default().into(),
        }
    }

    /// El `[[agent]]` que se guarda. Sin «usar en el orquestador», queda solo como acceso.
    fn to_agent(&self) -> Agent {
        let optional = |text: &str| self.orchestrate.then(|| text.trim().to_string());
        Agent {
            name: self.name.trim().into(),
            id: None,
            use_for: optional(&self.use_for),
            executor: optional(&self.template),
            command: self.command.trim().into(),
            worktree: self.worktree,
        }
    }

    /// «Probar» usa la plantilla del orquestador si está activa; si no, el comando.
    fn test_run(&self) -> Option<TestRun> {
        let template = self.orchestrate && !self.template.trim().is_empty();
        let command = if template {
            &self.template
        } else {
            &self.command
        };
        let command = command.trim();
        let name = match self.name.trim() {
            "" => "prueba",
            name => name,
        };
        (!command.is_empty()).then(|| TestRun {
            name: name.into(),
            command: command.into(),
            template,
        })
    }
}

/// Lo que impide guardar (errores) y lo que conviene saber antes (avisos).
#[derive(Default, Debug)]
struct Check {
    errors: Vec<String>,
    warnings: Vec<String>,
}

fn check(form: &Form, settings: &Settings) -> Check {
    let mut check = Check::default();
    let name = form.name.trim();
    let id = slug(name);
    if name.is_empty() {
        check.errors.push("Ponle un nombre.".into());
    } else if let Some(other) = settings.agents.iter().find(|a| {
        a.name.trim().to_lowercase() == name.to_lowercase()
            || a.id.clone().unwrap_or_else(|| slug(&a.name)) == id
    }) {
        check.errors.push(format!(
            "Ya tienes un agente con ese nombre o id: «{}».",
            other.name
        ));
    }
    if form.command.trim().is_empty() {
        check
            .errors
            .push("Escribe el comando que abre el agente.".into());
    }
    // En la paleta, un agente con el nombre de uno detectado y otro comando se vería dos veces.
    if PROVIDERS
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case(name) && p.command != form.command.trim())
    {
        check
            .warnings
            .push("Hay un agente conocido con ese nombre: otro nombre evita confundirlos.".into());
    }
    if !form.orchestrate {
        return check;
    }
    if form.use_for.trim().is_empty() {
        check
            .errors
            .push("Di para qué tareas conviene: con eso elige el planificador.".into());
    }
    if form.template.trim().is_empty() {
        check
            .errors
            .push("Escribe la plantilla para el orquestador.".into());
    } else if !form.template.contains("{prompt}") {
        check
            .warnings
            .push("La plantilla no tiene {prompt}: el prompt se agregará al final.".into());
    }
    if !name.is_empty() {
        if settings.executors.iter().any(|e| e.id == id) {
            check.errors.push(format!(
                "Ya tienes un [[executor]] con el id «{id}», que taparía a este agente."
            ));
        } else if catalog_with_sources(settings).iter().any(|(e, source)| {
            e.id == id && matches!(source, ExecutorSource::Claude | ExecutorSource::Detected)
        }) {
            check.warnings.push(format!(
                "Reemplazará al ejecutor «{id}» que Orion ya ofrece."
            ));
        }
    }
    check
}

/// Lo que se escribe en la pestaña de prueba.
fn test_command(shell: &ShellKind, run: &TestRun) -> String {
    if !run.template {
        return run
            .command
            .replace("{prompt}", &shell.quote_path(TEST_PROMPT));
    }
    let subtask = Subtask {
        title: String::new(),
        prompt: TEST_PROMPT.into(),
        executor: "prueba".into(),
        after: Vec::new(),
    };
    let executor = Executor {
        id: "prueba".into(),
        use_for: String::new(),
        command: run.command.clone(),
    };
    agent_command(shell, &subtask, &[executor])
}

impl Orion {
    /// Abre una pestaña con el agente del formulario. Cierra Configuración: con ella abierta, el
    /// teclado no llega a la terminal. Lo escrito en el formulario se conserva.
    pub(super) fn test_agent(&mut self, ctx: &egui::Context, run: TestRun) {
        let cwd = self
            .tabs
            .get(self.active)
            .and_then(|t| t.focused()?.cwd.clone());
        let name = run.name.clone();
        if self.launch_agent(ctx, cwd, &name, false, |shell| test_command(shell, &run)) {
            self.menu.open = false;
            self.show_toast(format!(
                "Probando {name}. Para guardarlo, vuelve a Configuración → Agentes."
            ));
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
        ctx.request_repaint();
    }
}

/// El contenido de la sección. Los cambios van a `new`: la ventana los aplica y los guarda.
pub(super) fn section(ui: &mut egui::Ui, new: &mut Settings, panel: &mut AgentsPanel) {
    known_agents(ui, new);
    local_models(ui, new, panel);
    add_form(ui, new, panel);
    own_entries(ui, new);
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).font(medium(14.0)));
    ui.add_space(4.0);
}

fn copy_button(ui: &mut egui::Ui, text: &str) {
    if ui.button("Copiar").on_hover_text(text).clicked() {
        ui.ctx().copy_text(text.into());
    }
}

/// Los agentes que Orion conoce: su estado y si el orquestador puede usarlos.
fn known_agents(ui: &mut egui::Ui, new: &mut Settings) {
    setting_row(
        ui,
        "Agentes detectados en el orquestador",
        "Ofrece solos los instalados que tienen plantilla",
        |ui| {
            ui.checkbox(&mut new.auto_executors, "");
        },
    );
    heading(ui, "Agentes conocidos");
    let installed = crate::agents::installed();
    let broken = crate::agents::broken();
    let catalog = catalog_with_sources(new);
    for (i, provider) in PROVIDERS.iter().enumerate() {
        let missing = broken.iter().find(|(j, _)| *j == i).map(|(_, path)| path);
        let ready = installed.contains(&i);
        let status = match missing {
            _ if ready => format!("✔ Instalado · {}", provider.about),
            Some(path) => format!(
                "⚠ Instalación rota: falta {}\nReinstalar: {}",
                path.display(),
                provider.install
            ),
            None => format!("No instalado · {}", provider.install),
        };
        setting_row(ui, provider.name, &status, |ui| {
            if !ready {
                copy_button(ui, provider.install);
                return;
            }
            if provider.executor.is_none() {
                ui.weak("Sin plantilla interactiva").on_hover_text(
                    "Se abre desde la paleta, pero no recibe tareas del orquestador",
                );
                return;
            }
            let id = slug(provider.name);
            let overridden = catalog.iter().any(|(e, source)| {
                e.id == id && matches!(source, ExecutorSource::Profile | ExecutorSource::Executor)
            });
            let mut on = overridden || !new.disabled_executors.contains(&id);
            let enabled = new.auto_executors && !overridden;
            let response = ui
                .add_enabled(
                    enabled,
                    egui::Checkbox::new(&mut on, "Usar en el orquestador"),
                )
                .on_disabled_hover_text(if overridden {
                    "Tu configuración ya define este ejecutor: quítalo abajo para usar el detectado"
                } else {
                    "Activa primero los agentes detectados en el orquestador"
                });
            if response.changed() {
                new.disabled_executors.retain(|d| *d != id);
                if !on {
                    new.disabled_executors.push(id);
                }
            }
        });
    }
}

/// Un botón por modelo de Ollama. Sin modelos (o sin Ollama) no se muestra nada.
fn local_models(ui: &mut egui::Ui, new: &mut Settings, panel: &mut AgentsPanel) {
    if panel
        .models_asked
        .is_none_or(|at| at.elapsed() >= MODELS_REFRESH)
    {
        panel.models_asked = Some(Instant::now());
        let (models, ctx) = (panel.models.clone(), ui.ctx().clone());
        std::thread::spawn(move || {
            let found = crate::local_models::ollama_models();
            *models.lock().unwrap_or_else(|e| e.into_inner()) = found;
            // La ventana de ajustes es inmediata: se redibuja junto con la principal.
            ctx.request_repaint_of(egui::ViewportId::ROOT);
            ctx.request_repaint_of(viewport_id());
        });
    }
    let models = panel
        .models
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if models.is_empty() {
        return;
    }
    heading(ui, "Modelos locales de Ollama");
    let installed = crate::agents::installed();
    let provider = |name: &str| PROVIDERS.iter().position(|p| p.name == name);
    let has = |name: &str| provider(name).is_some_and(|i| installed.contains(&i));
    let (opencode, aider) = (has("OpenCode"), has("Aider"));
    if !opencode {
        let text = if aider {
            "Tienes Aider, pero no puede recibir la tarea y seguir interactivo. \
             Para usar estos modelos como ejecutores, instala OpenCode."
        } else {
            "Para usar estos modelos como ejecutores, instala OpenCode."
        };
        ui.weak(text);
        if let Some(i) = provider("OpenCode") {
            let install = PROVIDERS[i].install;
            ui.horizontal_wrapped(|ui| {
                ui.monospace(install);
                copy_button(ui, install);
            });
        }
        ui.add_space(4.0);
    } else {
        ui.weak(
            "Usan OpenCode: configura en él el provider «ollama» y el modelo, \
             y deja Ollama en marcha.",
        );
        ui.add_space(4.0);
    }
    for model in &models {
        let offered = crate::local_models::executors_for(model, opencode, aider);
        let added = !offered.is_empty()
            && offered
                .iter()
                .all(|e| new.executors.iter().any(|old| old.id == e.id));
        let description = match offered.first() {
            Some(e) => format!("Ejecutor «{}»", e.id),
            None if opencode => "Su nombre tiene caracteres que no se admiten".into(),
            None => "Falta OpenCode".into(),
        };
        setting_row(ui, model, &description, |ui| {
            let label = if added {
                "Agregado"
            } else {
                "Agregar como ejecutor"
            };
            let enabled = !added && !offered.is_empty();
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                for executor in &offered {
                    if !new.executors.iter().any(|old| old.id == executor.id) {
                        new.executors.push(executor.clone());
                    }
                }
            }
        });
    }
}

fn text_row(ui: &mut egui::Ui, name: &str, description: &str, value: &mut String, hint: &str) {
    setting_row(ui, name, description, |ui| {
        let width = ui.available_width().min(300.0);
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(width),
        );
    });
}

fn add_form(ui: &mut egui::Ui, new: &mut Settings, panel: &mut AgentsPanel) {
    heading(ui, "Agregar agente");
    let before = panel.form.clone();
    setting_row(ui, "Plantilla", "Rellena los campos de abajo", |ui| {
        let name = |from: Option<usize>| from.map_or("Personalizado", |i| PROVIDERS[i].name);
        let mut from = panel.form.from;
        egui::ComboBox::from_id_salt("plantilla-agente")
            .width(180.0)
            .selected_text(name(from))
            .show_ui(ui, |ui| {
                for choice in (0..PROVIDERS.len()).map(Some).chain([None]) {
                    ui.selectable_value(&mut from, choice, name(choice));
                }
            });
        if from != panel.form.from {
            panel.form = Form::from_provider(from);
        }
    });
    let form = &mut panel.form;
    text_row(
        ui,
        "Nombre",
        "Como se verá en la paleta y el menú",
        &mut form.name,
        "Mi agente",
    );
    text_row(
        ui,
        "Comando",
        "Lo que se escribe en la shell para abrirlo",
        &mut form.command,
        "mi-agente",
    );
    setting_row(
        ui,
        "Worktree propio",
        "Lo abre en una rama y carpeta nuevas",
        |ui| {
            ui.checkbox(&mut form.worktree, "");
        },
    );
    setting_row(
        ui,
        "Usar en el orquestador",
        "Puede recibir subtareas de un plan",
        |ui| {
            ui.checkbox(&mut form.orchestrate, "");
        },
    );
    if form.orchestrate {
        text_row(
            ui,
            "Para qué sirve",
            "Le llega al planificador (use_for)",
            &mut form.use_for,
            "programar y escribir tests",
        );
        text_row(
            ui,
            "Plantilla con {prompt}",
            "Modo interactivo; que termine con commit",
            &mut form.template,
            "mi-agente --prompt {prompt}",
        );
    }
    if *form != before {
        panel.saved = None;
    }

    let form = panel.form.clone();
    if let Some(program) = form.command.split_whitespace().next() {
        program_status(ui, panel, program);
    }
    // La plantilla puede arrancar otro programa que el comando: ese es el que mira el orquestador.
    if form.orchestrate
        && let Some(program) = form.template.split_whitespace().next()
        && form.command.split_whitespace().next() != Some(program)
        && !matches!(locate(program), Some(Located::Ready(_)))
    {
        ui.colored_label(
            theme().ansi[3],
            format!("⚠ El orquestador no encuentra `{program}` (el de la plantilla)."),
        );
    }
    let check = check(&form, new);
    // Con el formulario sin tocar no se reclama nada.
    if form != Form::default() {
        for error in &check.errors {
            ui.colored_label(theme().ansi[1], error);
        }
        for warning in &check.warnings {
            ui.colored_label(theme().ansi[3], format!("⚠ {warning}"));
        }
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let run = form.test_run();
        if ui
            .add_enabled(run.is_some(), egui::Button::new("Probar"))
            .on_hover_text(format!(
                "Abre una pestaña en la carpeta actual y cierra esta ventana. \
                 Donde diga {{prompt}} va «{TEST_PROMPT}»."
            ))
            .clicked()
        {
            panel.test = run;
        }
        if ui
            .add_enabled(check.errors.is_empty(), egui::Button::new("Guardar"))
            .clicked()
        {
            let agent = form.to_agent();
            panel.saved = Some(agent.name.clone());
            new.agents.push(agent);
            panel.form = Form::default();
        }
        if let Some(name) = &panel.saved {
            ui.colored_label(theme().ansi[2], format!("«{name}» guardado"));
        }
    });
}

/// Si la primera palabra del comando es un programa que se puede ejecutar.
fn program_status(ui: &mut egui::Ui, panel: &mut AgentsPanel, program: &str) {
    let fresh = panel
        .located
        .as_ref()
        .is_some_and(|(name, at, _)| name == program && at.elapsed() < Duration::from_secs(3));
    if !fresh {
        panel.located = Some((program.into(), Instant::now(), locate(program)));
    }
    match panel
        .located
        .as_ref()
        .and_then(|(.., found)| found.as_ref())
    {
        Some(Located::Ready(path)) => ui.colored_label(
            theme().ansi[2],
            format!("✔ Programa encontrado: {}", path.display()),
        ),
        Some(Located::Broken { missing }) => ui.colored_label(
            theme().ansi[3],
            format!("⚠ `{program}` está roto: falta {}", missing.display()),
        ),
        None => ui.colored_label(theme().ansi[3], format!("⚠ No encuentro `{program}`.")),
    };
}

/// Un botón «Quitar» por cada `[[agent]]` y `[[executor]]` de la configuración.
fn own_entries(ui: &mut egui::Ui, new: &mut Settings) {
    heading(ui, "Tus agentes");
    if new.agents.is_empty() {
        ui.weak("Todavía no agregaste ninguno.");
    }
    let mut remove = None;
    for (i, agent) in new.agents.iter().enumerate() {
        let mut description = agent.command.clone();
        if agent.use_for.is_some() && agent.executor.is_some() {
            description += "\nTambién en el orquestador";
        }
        setting_row(ui, &agent.name, &description, |ui| {
            if ui.button("Quitar").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        new.agents.remove(i);
    }

    heading(ui, "Tus ejecutores del orquestador");
    if new.executors.is_empty() {
        ui.weak("Todavía no agregaste ninguno.");
    }
    let mut remove = None;
    for (i, executor) in new.executors.iter().enumerate() {
        setting_row(ui, &executor.id, &executor.command, |ui| {
            if ui
                .button("Quitar")
                .on_hover_text(&executor.use_for)
                .clicked()
            {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        new.executors.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::SHELLS;

    fn custom() -> Form {
        Form {
            name: " Mi agente ".into(),
            command: "mi-agente".into(),
            orchestrate: true,
            use_for: "programar".into(),
            template: "mi-agente --prompt {prompt}".into(),
            ..Form::default()
        }
    }

    #[test]
    fn every_provider_fills_a_form_that_can_be_saved() {
        for (i, provider) in PROVIDERS.iter().enumerate() {
            let form = Form::from_provider(Some(i));
            let found = check(&form, &Settings::default());
            assert!(found.errors.is_empty(), "{}: {found:?}", provider.name);
            assert_eq!(form.orchestrate, provider.executor.is_some());
        }
        assert!(Form::from_provider(None) == Form::default());
    }

    #[test]
    fn names_and_ids_cannot_repeat() {
        let mut settings = Settings {
            agents: vec![custom().to_agent()],
            ..Settings::default()
        };
        for name in ["MI AGENTE", "mi-agente"] {
            let form = Form {
                name: name.into(),
                ..custom()
            };
            assert_eq!(check(&form, &settings).errors.len(), 1, "{name}");
        }
        settings.agents[0].id = Some("otro".into());
        let form = Form {
            name: "Otro".into(),
            ..custom()
        };
        assert_eq!(check(&form, &settings).errors.len(), 1);
        // Un [[executor]] con ese id taparía al agente, pero solo importa para el orquestador.
        settings.executors = vec![Executor {
            id: "nuevo".into(),
            use_for: "x".into(),
            command: "x {prompt}".into(),
        }];
        let mut form = Form {
            name: "Nuevo".into(),
            ..custom()
        };
        assert_eq!(check(&form, &settings).errors.len(), 1);
        form.orchestrate = false;
        assert!(check(&form, &settings).errors.is_empty());
    }

    #[test]
    fn a_template_without_prompt_is_only_a_warning() {
        let form = Form {
            template: "mi-agente".into(),
            ..custom()
        };
        let found = check(&form, &Settings::default());
        assert!(found.errors.is_empty());
        assert!(found.warnings.iter().any(|w| w.contains("{prompt}")));
    }

    #[test]
    fn the_orchestrator_fields_are_saved_only_when_enabled() {
        let agent = custom().to_agent();
        assert_eq!(agent.name, "Mi agente");
        assert_eq!(agent.use_for.as_deref(), Some("programar"));
        assert!(agent.executor.is_some());
        let plain = Form {
            orchestrate: false,
            ..custom()
        }
        .to_agent();
        assert_eq!((plain.use_for, plain.executor), (None, None));
    }

    #[test]
    fn testing_replaces_the_prompt() {
        let shell = &SHELLS[0];
        let quoted = shell.quote_path(TEST_PROMPT);
        let run = custom().test_run().unwrap();
        assert_eq!(
            test_command(shell, &run),
            format!("mi-agente --prompt {quoted}")
        );
        let plain = Form {
            orchestrate: false,
            ..custom()
        };
        assert_eq!(test_command(shell, &plain.test_run().unwrap()), "mi-agente");
        assert!(Form::default().test_run().is_none());
    }
}
