//! La ventana del orquestador: escribes la tarea, Claude Code la planifica en un hilo aparte,
//! revisas qué subtareas y modelos eligió, y con un clic se abre una pestaña de agente por cada una.
//! Las que dependen de otras (`after`) esperan: arrancan, desde las ramas de aquellas, cuando sus
//! agentes terminan (los cierras con /exit después de revisar) y dejaron commits.

use super::{AgentStatus, Orion, Tab};
use crate::colors::theme;
use crate::orchestrator::{
    Dep, Gate, Planner, Subtask, agent_command, gate, has_commits, prepare_worktree, short_name,
};
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

/// Cómo va una subtarea de un plan lanzado.
enum Run {
    /// Espera a sus dependencias.
    Waiting,
    /// Su agente está abierto en el panel `pane` (un id estable); `base` es el commit del que partió.
    Working {
        pane: u64,
        branch: String,
        base: String,
    },
    /// Su agente terminó: con qué código y si dejó commits propios.
    Done {
        branch: String,
        code: Option<i32>,
        commits: bool,
    },
    /// Se cerró su pestaña antes de que el agente terminara.
    Closed,
    /// No arrancó, y por qué.
    Blocked(String),
}

impl Run {
    fn dep(&self) -> Dep {
        match self {
            Run::Waiting | Run::Working { .. } => Dep::Pending,
            &Run::Done { code, commits, .. } => Dep::Done { code, commits },
            Run::Closed => Dep::Closed,
            Run::Blocked(_) => Dep::Blocked,
        }
    }

    /// Lo que muestra la ventana, y si es un problema.
    fn label(&self, title: &str) -> (String, bool) {
        match self {
            Run::Waiting => ("esperando".into(), false),
            Run::Working { .. } => ("trabajando".into(), false),
            Run::Blocked(why) => (format!("bloqueada: {why}"), true),
            // Terminó o se cerró: el mismo motivo que verían las que dependen de ella.
            finished => match gate(&[(title, finished.dep())]) {
                Gate::Blocked(why) => (why, true),
                _ => ("lista".into(), false),
            },
        }
    }
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
    /// El plan lanzado y cómo va cada subtarea; vacío = ninguno en curso.
    plan: Vec<(Subtask, Run)>,
}

impl Default for Orchestrate {
    fn default() -> Self {
        Self {
            open: false,
            task: String::new(),
            dir: PathBuf::new(),
            step: Arc::new(Mutex::new(Step::Writing)),
            planner: Planner::default(),
            plan: Vec::new(),
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

/// "después de 1 y 2", con los números que muestra la ventana (desde 1).
fn after_text(after: &[usize]) -> String {
    let n: Vec<String> = after.iter().map(|j| (j + 1).to_string()).collect();
    format!("después de {}", n.join(", "))
}

impl Orion {
    /// Abre la ventana para la pestaña actual, que tiene que estar dentro de un repo git.
    pub(super) fn open_orchestrate(&mut self) {
        // Con un plan en camino, listo o en curso, se vuelve a él: es de su repo, no del de la pestaña actual.
        let o = &mut self.orchestrate;
        if !o.plan.is_empty() || matches!(o.step(), Step::Planning | Step::Ready(_)) {
            o.open = true;
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
        let (mut plan_now, mut launch, mut back, mut forget) = (false, false, false, false);
        let mut open = !escape;
        let model = |s: &Subtask| {
            egui::RichText::new(short_name(&s.executor))
                .monospace()
                .color(theme().ansi[4])
        };
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
                if !o.plan.is_empty() {
                    for (i, (s, run)) in o.plan.iter().enumerate() {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(format!("{}.", i + 1));
                            ui.label(model(s));
                            ui.label(&s.title).on_hover_text(&s.prompt);
                            let (text, bad) = run.label(&s.title);
                            let color = match (bad, run) {
                                (true, _) => theme().ansi[1],
                                (false, Run::Done { .. }) => theme().ansi[2],
                                _ => ui.visuals().weak_text_color(),
                            };
                            ui.colored_label(color, text);
                        });
                    }
                    ui.weak(
                        "Revisa lo que hizo cada agente y ciérralo con /exit: las subtareas que \
                         dependen de él arrancan desde su rama si dejó commits.",
                    );
                    forget = ui
                        .button("Olvidar el plan")
                        .on_hover_text("Las pestañas y ramas quedan; solo deja de lanzar las que faltan")
                        .clicked();
                    return;
                }
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
                        for (i, s) in subtasks.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.weak(format!("{}.", i + 1));
                                ui.label(model(s));
                                ui.label(&s.title).on_hover_text(&s.prompt);
                                if !s.after.is_empty() {
                                    ui.weak(after_text(&s.after));
                                }
                            });
                        }
                        ui.weak("Pasa el mouse por una subtarea para ver su prompt.");
                        if subtasks.iter().any(|s| !s.after.is_empty()) {
                            ui.weak(
                                "Las que van «después de» otra arrancan cuando cierres esa \
                                 (con /exit) y haya hecho commit.",
                            );
                        }
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
        if forget {
            (o.plan, o.open) = (Vec::new(), false);
            return None;
        }
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
            let catalog = crate::orchestrator::catalog(&self.settings.executors);
            let (task, dir, step, planner, ctx) = (
                o.task.clone(),
                o.dir.clone(),
                o.step.clone(),
                o.planner.clone(),
                ctx.clone(),
            );
            std::thread::spawn(move || {
                let result = match crate::orchestrator::plan(&task, &dir, &planner, &catalog) {
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

    /// Lanza el plan: las subtareas sin dependencias arrancan ya; las demás esperan su ronda.
    pub(super) fn launch_plan(&mut self, ctx: &egui::Context, subtasks: Vec<Subtask>) {
        self.orchestrate.plan = subtasks.into_iter().map(|s| (s, Run::Waiting)).collect();
        self.advance_plan(ctx);
    }

    /// Una vez por frame: anota qué agentes terminaron y lanza las subtareas cuyas dependencias ya
    /// están listas (o las bloquea, con un aviso). Sin plan en curso no hace nada.
    pub(super) fn advance_plan(&mut self, ctx: &egui::Context) {
        let (mut launched, mut waiting, mut news) = (Vec::new(), 0, Vec::new());
        for i in 0..self.orchestrate.plan.len() {
            let next = match &self.orchestrate.plan[i].1 {
                Run::Working { pane, branch, base } => self.check_agent(*pane, branch, base),
                Run::Waiting => {
                    // Las dependencias tienen índices menores: ya se actualizaron en esta pasada.
                    let plan = &self.orchestrate.plan;
                    let s = &plan[i].0;
                    let deps: Vec<(&str, Dep)> = (s.after.iter())
                        .map(|&j| (plan[j].0.title.as_str(), plan[j].1.dep()))
                        .collect();
                    let branches: Vec<String> = (s.after.iter())
                        .filter_map(|&j| match &plan[j].1 {
                            Run::Done { branch, .. } => Some(branch.clone()),
                            _ => None,
                        })
                        .collect();
                    let (title, gate) = (s.title.clone(), gate(&deps));
                    match gate {
                        Gate::Wait => {
                            waiting += 1;
                            None
                        }
                        Gate::Blocked(why) => {
                            news.push(format!("«{title}» quedó bloqueada: {why}"));
                            Some(Run::Blocked(why))
                        }
                        Gate::Ready => {
                            let run = self.start_subtask(ctx, i, &branches);
                            match &run {
                                Run::Blocked(why) => {
                                    news.push(format!("«{title}» no arrancó: {why}"))
                                }
                                _ => launched.push(title),
                            }
                            Some(run)
                        }
                    }
                }
                _ => None,
            };
            if let Some(run) = next {
                self.orchestrate.plan[i].1 = run;
            }
        }
        if !launched.is_empty() {
            let mut text = match launched.as_slice() {
                [one] => format!("«{one}» arrancó en su rama"),
                many => format!("{} agentes arrancaron, cada uno en su rama", many.len()),
            };
            if waiting > 0 {
                text += &format!("; {waiting} esperan a otras subtareas (las ves en Orquestar)");
            }
            news.insert(0, text);
        }
        if !news.is_empty() {
            self.show_toast(news.join(" · "));
        }
    }

    /// Si el agente del panel `pane` terminó (o se cerró su pestaña), su nuevo estado.
    fn check_agent(&self, pane: u64, branch: &str, base: &str) -> Option<Run> {
        let mine = |t: &&Tab| t.agent_run.as_ref().is_some_and(|(_, p)| *p == pane);
        match self.tabs.iter().find(mine).and_then(Tab::agent_status) {
            None => Some(Run::Closed),
            Some(AgentStatus::Done(code)) => Some(Run::Done {
                branch: branch.to_string(),
                code,
                commits: has_commits(&self.orchestrate.dir, base, branch),
            }),
            Some(_) => None,
        }
    }

    /// Abre la subtarea `i` en un worktree que parte de las ramas de sus dependencias.
    fn start_subtask(&mut self, ctx: &egui::Context, i: usize, branches: &[String]) -> Run {
        let s = self.orchestrate.plan[i].0.clone();
        let repo = self.orchestrate.dir.clone();
        let (path, branch, base) = match prepare_worktree(&repo, &s.title, branches) {
            Ok(ready) => ready,
            Err(e) => return Run::Blocked(e),
        };
        let catalog = crate::orchestrator::catalog(&self.settings.executors);
        let command = |shell: &crate::pty::ShellKind| agent_command(shell, &s, &catalog);
        if !self.launch_agent(ctx, Some(path.clone()), &s.title, false, command) {
            return Run::Blocked("no pude abrir su pestaña".into());
        }
        // `launch_agent` la tomó por una terminal en esa carpeta: es un worktree de `repo`.
        let tab = &mut self.tabs[self.active];
        (tab.repo, tab.worktree) = (Some(repo), Some(path));
        let pane = tab.agent_run.as_ref().map_or(tab.focus, |(_, p)| *p);
        Run::Working { pane, branch, base }
    }
}
