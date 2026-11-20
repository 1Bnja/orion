//! La ventana del orquestador: escribes la tarea, Claude Code la planifica en un hilo aparte,
//! revisas qué subtareas y modelos eligió, y con un clic se abre una pestaña de agente por cada una.

use super::Orion;
use crate::colors::theme;
use crate::orchestrator::{Planner, Subtask, short_name};
use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
enum Step {
    Writing,
    Planning,
    Ready(Vec<Subtask>),
    Failed(String),
}

pub(super) struct Orchestrate {
    pub(super) open: bool,
    task: String,
    /// El repo donde se planifica y se crean los worktrees (el de la pestaña desde donde lo abriste).
    dir: PathBuf,
    /// Lo escribe el hilo que planifica. Cancelar pone uno nuevo: el hilo viejo escribe en el suyo y nadie lo mira.
    step: Arc<Mutex<Step>>,
    /// El `claude -p` que planifica: Cancelar lo corta para que no siga gastando uso.
    planner: Planner,
}

impl Default for Orchestrate {
    fn default() -> Self {
        Self {
            open: false,
            task: String::new(),
            dir: PathBuf::new(),
            step: Arc::new(Mutex::new(Step::Writing)),
            planner: Planner::default(),
        }
    }
}

impl Orchestrate {
    fn step(&self) -> Step {
        self.step.lock().unwrap().clone()
    }

    fn set(&mut self, step: Step) {
        self.step = Arc::new(Mutex::new(step));
    }
}

impl Orion {
    /// Abre la ventana para la pestaña actual, que tiene que estar dentro de un repo git.
    pub(super) fn open_orchestrate(&mut self) {
        // Con un plan en camino o listo, se vuelve a él: es de su repo, no del de la pestaña actual.
        if matches!(self.orchestrate.step(), Step::Planning | Step::Ready(_)) {
            self.orchestrate.open = true;
            return;
        }
        let root = match self.session().cwd.clone() {
            None => Err(
                "abre el orquestador desde una pestaña de PowerShell o zsh dentro del repo".into(),
            ),
            Some(cwd) => crate::agents::repo_root(&cwd),
        };
        match root {
            Ok(dir) => {
                let o = &mut self.orchestrate;
                (o.open, o.dir) = (true, dir);
                o.set(Step::Writing);
            }
            Err(e) => self.show_toast(format!("Orquestar: {e}")),
        }
    }

    /// Dibuja la ventana si está abierta. Devuelve el plan cuando aprietas "Lanzar".
    pub(super) fn orchestrate_window(&mut self, ctx: &egui::Context) -> Option<Vec<Subtask>> {
        let o = &mut self.orchestrate;
        if !o.open {
            return None;
        }
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let step = o.step();
        let (mut plan_now, mut launch, mut back) = (false, false, false);
        let mut open = !escape;
        egui::Window::new("Orquestar tarea con agentes")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 48.0))
            .min_width(480.0)
            .max_width(480.0)
            .show(ctx, |ui| {
                ui.weak(format!("Repo: {}", o.dir.display()));
                ui.add_space(4.0);
                match &step {
                    Step::Writing => {
                        let edit = egui::TextEdit::multiline(&mut o.task)
                            .hint_text("¿Qué hay que hacer? Claude Code lo divide en subtareas, \
                                        elige un modelo para cada una y abre un agente por subtarea, \
                                        cada uno en su propia rama.")
                            .desired_rows(5)
                            .desired_width(f32::INFINITY);
                        ui.add(edit).request_focus();
                        let enter = ui.input_mut(|i| {
                            i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)
                        });
                        ui.horizontal(|ui| {
                            let ready = !o.task.trim().is_empty();
                            let button = ui.add_enabled(ready, egui::Button::new("Planificar"));
                            plan_now = ready && (button.clicked() || enter);
                            ui.weak(keys!("+Enter"));
                        });
                    }
                    Step::Planning => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Claude Code está leyendo el repo y planificando…");
                        });
                        ui.weak("Puede tardar un par de minutos. Puedes cerrar esta ventana y volver.");
                        back = ui.button("Cancelar").clicked();
                    }
                    Step::Ready(subtasks) => {
                        for s in subtasks {
                            ui.horizontal(|ui| {
                                let model = egui::RichText::new(short_name(&s.model))
                                    .monospace()
                                    .color(theme().ansi[4]);
                                ui.label(model);
                                ui.label(&s.title).on_hover_text(&s.prompt);
                            });
                        }
                        ui.weak("Pasa el mouse por una subtarea para ver su prompt.");
                        ui.horizontal(|ui| {
                            let n = subtasks.len();
                            let text = if n == 1 {
                                "Lanzar 1 agente".into()
                            } else {
                                format!("Lanzar {n} agentes")
                            };
                            launch = ui.button(text).clicked();
                            back = ui.button("Volver").clicked();
                        });
                    }
                    Step::Failed(e) => {
                        ui.colored_label(theme().ansi[1], e);
                        back = ui.button("Volver").clicked();
                    }
                }
            });
        o.open = open;
        if back {
            // Volver durante la planificación es cancelarla: se corta el proceso, no solo la vista.
            crate::orchestrator::cancel(&o.planner);
            o.set(Step::Writing);
        }
        if plan_now {
            o.set(Step::Planning);
            // ponytail: si se cancela en el instante antes de que arranque el proceso, igual corre;
            // su resultado se descarta (el `step` ya es otro).
            o.planner = Planner::default();
            let (task, dir, step, planner, ctx) = (
                o.task.clone(),
                o.dir.clone(),
                o.step.clone(),
                o.planner.clone(),
                ctx.clone(),
            );
            std::thread::spawn(move || {
                let result = match crate::orchestrator::plan(&task, &dir, &planner) {
                    Ok(subtasks) => Step::Ready(subtasks),
                    Err(e) => Step::Failed(e),
                };
                *step.lock().unwrap() = result;
                ctx.request_repaint();
            });
        }
        let Step::Ready(subtasks) = step else {
            return None;
        };
        if !launch {
            return None;
        }
        (o.open, o.task) = (false, String::new());
        o.set(Step::Writing);
        Some(subtasks)
    }

    /// Una pestaña de agente por subtarea, cada una en su worktree y con su modelo.
    pub(super) fn launch_plan(&mut self, ctx: &egui::Context, subtasks: Vec<Subtask>) {
        let dir = Some(self.orchestrate.dir.clone());
        let mut opened = 0;
        for s in &subtasks {
            let command =
                |shell: &crate::pty::ShellKind| crate::orchestrator::agent_command(shell, s);
            if !self.launch_agent(ctx, dir.clone(), &s.title, true, command) {
                return; // ya avisó por qué; las que se abrieron quedan
            }
            opened += 1;
        }
        self.show_toast(format!("{opened} agentes trabajando, cada uno en su rama"));
    }
}
