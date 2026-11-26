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
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
enum Step {
    Writing,
    Planning,
    /// El plan, los ids de los ejecutores instalados que se le ofrecieron (para cambiar el de cada
    /// subtarea) y una nota de cuáles no se ofrecieron y por qué.
    Ready(Vec<Subtask>, Vec<String>, String),
    Failed(String),
}

/// Cómo va una subtarea de un plan lanzado.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
enum Run {
    /// Espera a sus dependencias.
    Waiting,
    /// Su agente está abierto en el panel `pane` (un id estable); `base` es el commit del que partió.
    Working {
        pane: u64,
        branch: String,
        base: String,
    },
    /// Su agente terminó: con qué código y si dejó commits propios (mirado al terminar; con
    /// «Reintentar» se vuelve a mirar).
    Done {
        branch: String,
        base: String,
        code: Option<i32>,
        commits: bool,
    },
    /// Se cerró su pestaña antes de que el agente terminara.
    Closed { branch: String, base: String },
    /// No arrancó, y por qué.
    Blocked(String),
}

impl Run {
    fn dep(&self) -> Dep {
        match self {
            Run::Waiting | Run::Working { .. } => Dep::Pending,
            &Run::Done { code, commits, .. } => Dep::Done { code, commits },
            Run::Closed { .. } => Dep::Closed,
            Run::Blocked(_) => Dep::Blocked,
        }
    }

    /// Terminó bien y dejó commits: no queda nada por hacer con ella.
    fn succeeded(&self) -> bool {
        matches!(self, Run::Done { .. }) && !self.can_retry()
    }

    /// Si se puede reintentar: quedó bloqueada, o terminó (o se cerró) de una forma que bloquea
    /// a las que dependen de ella.
    fn can_retry(&self) -> bool {
        match self {
            Run::Blocked(_) | Run::Closed { .. } => true,
            Run::Done { .. } => matches!(gate(&[("", self.dep())]), Gate::Blocked(_)),
            Run::Waiting | Run::Working { .. } => false,
        }
    }

    /// «Reintentar». Una bloqueada vuelve a esperar a sus dependencias. Una que terminó o se
    /// cerró se da por buena (su código ya no cuenta) y se vuelve a mirar si su rama tiene
    /// commits: pueden haber llegado después, por ejemplo si el agente se cortó y el commit
    /// se hizo a mano. `commits(base, rama)` es `has_commits`.
    fn retry(&mut self, commits: impl FnOnce(&str, &str) -> bool) {
        match self {
            Run::Blocked(_) => *self = Run::Waiting,
            Run::Done { branch, base, .. } | Run::Closed { branch, base } => {
                let commits = commits(base, branch);
                let (branch, base) = (std::mem::take(branch), std::mem::take(base));
                *self = Run::Done {
                    branch,
                    base,
                    code: None,
                    commits,
                };
            }
            Run::Waiting | Run::Working { .. } => {}
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
    /// Un plan recuperado de la vez anterior no lanza nada hasta «Reanudar».
    paused: bool,
    /// Lo último guardado en orchestrate.json.
    saved: Vec<(Subtask, Run)>,
    /// Las ramas de agentes que siguen trabajando y ya tienen commits.
    committed: HashSet<String>,
    /// La búsqueda de esos commits en camino, y cuándo se pidió la última.
    checking: Option<Receiver<Vec<String>>>,
    checked: Instant,
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
            paused: false,
            saved: Vec::new(),
            committed: HashSet::new(),
            checking: None,
            checked: Instant::now(),
        }
    }
}

/// Cada cuánto se mira si un agente que sigue trabajando ya hizo commit.
const CHECK_EVERY: Duration = Duration::from_secs(10);

/// Lo que se guarda en orchestrate.json, al lado de config.toml.
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    dir: PathBuf,
    plan: Vec<(Subtask, Run)>,
}

fn stored_path() -> Option<PathBuf> {
    // Los tests no leen ni pisan el plan del usuario.
    if cfg!(test) {
        return None;
    }
    crate::config::config_path().map(|p| p.with_file_name("orchestrate.json"))
}

/// Un plan leído del disco: los agentes que estaban trabajando ya no tienen pestaña.
fn interrupted(mut plan: Vec<(Subtask, Run)>) -> Vec<(Subtask, Run)> {
    for (_, run) in &mut plan {
        if let Run::Working { branch, base, .. } = run {
            *run = Run::Closed {
                branch: std::mem::take(branch),
                base: std::mem::take(base),
            };
        }
    }
    plan
}

impl Orchestrate {
    /// El de siempre, con el plan que quedó sin terminar la última vez, en pausa.
    pub(super) fn restore() -> Self {
        let text = stored_path().and_then(|p| std::fs::read_to_string(p).ok());
        let Some(stored) = text.and_then(|t| serde_json::from_str::<Stored>(&t).ok()) else {
            return Self::default();
        };
        let unfinished = stored.plan.iter().any(|(_, run)| !run.succeeded());
        if !unfinished {
            return Self::default();
        }
        let plan = interrupted(stored.plan);
        Self {
            dir: stored.dir,
            saved: plan.clone(),
            plan,
            paused: true,
            ..Self::default()
        }
    }

    pub(super) fn paused(&self) -> bool {
        self.paused
    }

    /// Guarda el plan si cambió. Si no se puede escribir, solo se pierde al cerrar Orion.
    fn persist(&mut self) {
        if self.plan == self.saved {
            return;
        }
        self.saved = self.plan.clone();
        let Some(path) = stored_path() else { return };
        if self.plan.is_empty() {
            let _ = std::fs::remove_file(path);
            return;
        }
        let stored = Stored {
            dir: self.dir.clone(),
            plan: self.plan.clone(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&stored) {
            let _ = std::fs::write(path, text);
        }
    }

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

/// El atajo a Configuración → Agentes, para sumar ejecutores sin editar config.toml.
fn add_agents_button(ui: &mut egui::Ui) -> bool {
    ui.button("Agregar agentes…")
        .on_hover_text("Abre Configuración en la sección Agentes")
        .clicked()
}

impl Orion {
    /// Abre la ventana para la pestaña actual, que tiene que estar dentro de un repo git.
    pub(super) fn open_orchestrate(&mut self) {
        // Con un plan en camino, listo o en curso, se vuelve a él: es de su repo, no del de la pestaña actual.
        let o = &mut self.orchestrate;
        if !o.plan.is_empty() || matches!(o.step(), Step::Planning | Step::Ready(..)) {
            o.open = true;
            return;
        }
        let root = match self.session().and_then(|s| s.cwd.clone()) {
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
        let mut add_agents = false;
        // Elegiste otro ejecutor para una subtarea: (índice, id).
        let mut pick: Option<(usize, String)> = None;
        // Diste por terminada a mano la subtarea con ese índice.
        let mut finish: Option<usize> = None;
        // Pediste reintentar la subtarea con ese índice.
        let mut retry: Option<usize> = None;
        let (mut resume, mut clean) = (false, false);
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
                            if let Run::Working { branch, .. } = run
                                && o.committed.contains(branch)
                            {
                                ui.colored_label(theme().ansi[3], "ya hizo commit")
                                    .on_hover_text(
                                        "Si ya terminó, ciérrala con /exit o con «Ya terminó»",
                                    );
                            }
                            // Plan B si la shell no avisa cuándo termina el agente (cmd, WSL).
                            if matches!(run, Run::Working { .. })
                                && ui
                                    .small_button("Ya terminó")
                                    .on_hover_text(
                                        "Darla por terminada: las que dependen de ella \
                                         arrancan si dejó commits en su rama",
                                    )
                                    .clicked()
                            {
                                finish = Some(i);
                            }
                            if run.can_retry()
                                && ui
                                    .small_button("Reintentar")
                                    .on_hover_text(
                                        "Una bloqueada vuelve a esperar a sus dependencias; una \
                                         que terminó mal se da por buena y se vuelve a mirar si \
                                         su rama tiene commits",
                                    )
                                    .clicked()
                            {
                                retry = Some(i);
                            }
                        });
                    }
                    ui.weak(
                        "Revisa lo que hizo cada agente y ciérralo con /exit: las subtareas que \
                         dependen de él arrancan desde su rama si dejó commits.",
                    );
                    if o.paused {
                        ui.colored_label(
                            theme().ansi[3],
                            "Plan recuperado de la vez anterior, en pausa. Usa «Reintentar» en \
                             las subtareas que ya hicieron su commit y después «Reanudar».",
                        );
                        resume = ui
                            .button("Reanudar")
                            .on_hover_text("Vuelve a lanzar las que esperan, si pueden arrancar")
                            .clicked();
                    }
                    if o.plan.iter().all(|(_, run)| run.succeeded()) {
                        clean = ui
                            .button("Limpiar worktrees")
                            .on_hover_text(
                                "Borra los worktrees de Orion que ninguna pestaña usa y no \
                                 tienen cambios sin guardar; sus ramas, solo si ya se fusionaron",
                            )
                            .clicked();
                    }
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
                            add_agents = add_agents_button(ui);
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
                    Step::Ready(subtasks, offered, note) => {
                        for (i, s) in subtasks.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.weak(format!("{}.", i + 1));
                                // La recomendación del plan, cambiable por otro ejecutor instalado.
                                egui::ComboBox::from_id_salt(("ejecutor", i))
                                    .selected_text(model(s))
                                    .show_ui(ui, |ui| {
                                        for id in offered {
                                            let on = *id == s.executor;
                                            if ui.selectable_label(on, short_name(id)).clicked() {
                                                pick = Some((i, id.clone()));
                                            }
                                        }
                                    })
                                    .response
                                    .on_hover_text("Ejecutor de esta subtarea");
                                ui.label(&s.title).on_hover_text(&s.prompt);
                                if !s.after.is_empty() {
                                    ui.weak(after_text(&s.after));
                                }
                            });
                        }
                        ui.weak("Pasa el mouse por una subtarea para ver su prompt.");
                        ui.weak(note);
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
                            add_agents = add_agents_button(ui);
                        });
                    }
                    Step::Failed(e) => {
                        ui.colored_label(theme().ansi[1], e);
                        back = ui.button("Volver").clicked();
                    }
                }
            });
        o.open = open;
        if add_agents {
            // La tarea y el plan se conservan: al volver a Orquestar siguen ahí.
            o.open = false;
            self.open_agents_settings(ctx);
            return None;
        }
        if let Some((i, id)) = pick
            && let Step::Ready(subtasks, ..) = &mut *o.step.lock().unwrap()
            && let Some(s) = subtasks.get_mut(i)
        {
            s.executor = id;
        }
        if let Some(i) = finish
            && let Some((_, run)) = o.plan.get_mut(i)
            && let Run::Working { branch, base, .. } = run
        {
            let commits = has_commits(&o.dir, base, branch);
            let (branch, base) = (std::mem::take(branch), std::mem::take(base));
            *run = Run::Done {
                branch,
                base,
                code: None,
                commits,
            };
        }
        if let Some(i) = retry {
            // Reintentar una bloqueada también vuelve a mirar las que terminaron y la bloquearon:
            // si no, volvería a bloquearse en el próximo frame. Primero ellas, después la pedida.
            let after = o.plan.get(i).map(|(s, _)| s.after.clone());
            for j in after.unwrap_or_default().into_iter().chain([i]) {
                let dir = &o.dir;
                if let Some((_, run)) = o.plan.get_mut(j)
                    && (j == i || matches!(run, Run::Done { .. } | Run::Closed { .. }))
                    && run.can_retry()
                {
                    run.retry(|base, branch| has_commits(dir, base, branch));
                }
            }
        }
        if resume {
            o.paused = false;
        }
        if forget {
            (o.plan, o.open, o.paused) = (Vec::new(), false, false);
            o.persist();
            return None;
        }
        if clean {
            self.clean_worktrees();
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
            let catalog = crate::orchestrator::catalog(&self.settings);
            let extra = self.settings.executors.len();
            let (task, dir, step, planner, ctx) = (
                o.task.clone(),
                o.dir.clone(),
                o.step.clone(),
                o.planner.clone(),
                ctx.clone(),
            );
            std::thread::spawn(move || {
                let result = match crate::orchestrator::plan(&task, &dir, &planner, &catalog) {
                    Ok((subtasks, offered)) => {
                        // Por qué el menú ofrece lo que ofrece: cuántos vienen de config.toml y cuáles faltan.
                        let missing = crate::orchestrator::unavailable(&catalog);
                        let mut note = format!("Ejecutores de config.toml: {extra}.");
                        if !missing.is_empty() {
                            note += &format!(" No disponibles: {}.", missing.join("; "));
                        }
                        Step::Ready(subtasks, offered, note)
                    }
                    Err(e) => Step::Failed(e),
                };
                *step.lock().unwrap() = result;
                ctx.request_repaint();
            });
        }
        let Step::Ready(subtasks, ..) = step else {
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
        self.orchestrate.paused = false;
        self.orchestrate.committed.clear();
        self.advance_plan(ctx);
    }

    /// Una vez por frame: anota qué agentes terminaron y lanza las subtareas cuyas dependencias ya
    /// están listas (o las bloquea, con un aviso). Sin plan en curso no hace nada.
    pub(super) fn advance_plan(&mut self, ctx: &egui::Context) {
        if self.orchestrate.paused {
            self.orchestrate.persist(); // un «Reintentar» en pausa también se guarda
            return;
        }
        let (mut launched, mut waiting, mut news) = (Vec::new(), 0, Vec::new());
        news.extend(self.check_commits(ctx));
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
        self.orchestrate.persist();
    }

    /// Cada `CHECK_EVERY` mira, en un hilo, qué agentes que siguen trabajando ya hicieron
    /// commit. Devuelve un aviso por cada uno que aparece.
    fn check_commits(&mut self, ctx: &egui::Context) -> Vec<String> {
        let o = &mut self.orchestrate;
        let mut news = Vec::new();
        if let Some(rx) = &o.checking {
            match rx.try_recv() {
                Ok(branches) => {
                    for (s, run) in &o.plan {
                        if let Run::Working { branch, .. } = run
                            && branches.contains(branch)
                            && o.committed.insert(branch.clone())
                        {
                            news.push(format!("«{}» ya hizo commit en su rama", s.title));
                        }
                    }
                    o.checking = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => o.checking = None,
            }
        }
        let pending: Vec<(String, String)> = (o.plan.iter())
            .filter_map(|(_, run)| match run {
                Run::Working { branch, base, .. } if !o.committed.contains(branch) => {
                    Some((base.clone(), branch.clone()))
                }
                _ => None,
            })
            .collect();
        if pending.is_empty() || o.checking.is_some() {
            return news;
        }
        if o.checked.elapsed() >= CHECK_EVERY {
            let (tx, rx) = mpsc::channel();
            let (dir, ctx) = (o.dir.clone(), ctx.clone());
            std::thread::spawn(move || {
                let found = pending.into_iter();
                let found = found.filter(|(base, branch)| has_commits(&dir, base, branch));
                let _ = tx.send(found.map(|(_, branch)| branch).collect());
                ctx.request_repaint();
            });
            o.checking = Some(rx);
            o.checked = Instant::now();
        }
        ctx.request_repaint_after(CHECK_EVERY);
        news
    }

    /// Si el agente del panel `pane` terminó (o se cerró su pestaña), su nuevo estado.
    fn check_agent(&self, pane: u64, branch: &str, base: &str) -> Option<Run> {
        let mine = |t: &&Tab| t.agent_run == Some(pane);
        match self.tabs.iter().find(mine).and_then(Tab::agent_status) {
            None => Some(Run::Closed {
                branch: branch.to_string(),
                base: base.to_string(),
            }),
            Some(AgentStatus::Done(code)) => Some(Run::Done {
                branch: branch.to_string(),
                base: base.to_string(),
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
        let catalog = crate::orchestrator::catalog(&self.settings);
        let command = |shell: &crate::pty::ShellKind| agent_command(shell, &s, &catalog);
        if !self.launch_agent(ctx, Some(path.clone()), &s.title, false, command) {
            return Run::Blocked("no pude abrir su pestaña".into());
        }
        // `launch_agent` la tomó por una terminal en esa carpeta: es un worktree de `repo`.
        let tab = &mut self.tabs[self.active];
        (tab.repo, tab.worktree) = (Some(repo), Some(path));
        let pane = tab.agent_run.unwrap_or(tab.focus);
        Run::Working { pane, branch, base }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(code: Option<i32>, commits: bool) -> Run {
        Run::Done {
            branch: "orion/a-1".into(),
            base: "abc".into(),
            code,
            commits,
        }
    }

    #[test]
    fn a_saved_plan_comes_back_without_its_agents() {
        let subtask = |title: &str, after: Vec<usize>| Subtask {
            title: title.into(),
            prompt: "p".into(),
            executor: "e".into(),
            after,
        };
        let working = Run::Working {
            pane: 7,
            branch: "orion/a-1".into(),
            base: "abc".into(),
        };
        let stored = Stored {
            dir: PathBuf::from("repo"),
            plan: vec![
                (subtask("a", vec![]), working),
                (subtask("b", vec![]), done(Some(0), true)),
                (subtask("c", vec![0, 1]), Run::Waiting),
            ],
        };
        let text = serde_json::to_string(&stored).unwrap();
        let read: Stored = serde_json::from_str(&text).unwrap();
        assert_eq!(read.plan, stored.plan);

        let plan = interrupted(read.plan);
        // La que trabajaba queda cerrada (se puede reintentar); las demás, igual.
        assert!(matches!(&plan[0].1, Run::Closed { branch, .. } if branch == "orion/a-1"));
        assert!(plan[0].1.can_retry() && !plan[0].1.succeeded());
        assert!(plan[1].1.succeeded());
        assert_eq!(plan[2].1, Run::Waiting);
    }

    #[test]
    fn retry_rechecks_commits_that_arrived_late() {
        // El agente se cortó antes del commit; el commit se hizo después, a mano.
        let mut run = done(Some(1), false);
        assert!(run.can_retry());
        let mut asked = None;
        run.retry(|base, branch| {
            asked = Some((base.to_string(), branch.to_string()));
            true
        });
        assert_eq!(asked, Some(("abc".into(), "orion/a-1".into())));
        // Se da por buena: ya no bloquea a las que dependen de ella.
        assert_eq!(
            run.dep(),
            Dep::Done {
                code: None,
                commits: true
            }
        );
        assert!(!run.can_retry());
    }

    #[test]
    fn retry_without_commits_still_blocks() {
        let mut run = done(None, false);
        run.retry(|_, _| false);
        assert!(run.can_retry());
        assert!(matches!(gate(&[("a", run.dep())]), Gate::Blocked(_)));
    }

    #[test]
    fn retry_a_closed_tab_or_a_blocked_subtask() {
        let mut closed = Run::Closed {
            branch: "orion/a-1".into(),
            base: "abc".into(),
        };
        assert!(closed.can_retry());
        closed.retry(|_, _| true);
        assert_eq!(
            closed.dep(),
            Dep::Done {
                code: None,
                commits: true
            }
        );

        let mut blocked = Run::Blocked("«a» terminó sin hacer commit en su rama".into());
        assert!(blocked.can_retry());
        blocked.retry(|_, _| unreachable!("una bloqueada no mira commits"));
        assert!(matches!(blocked, Run::Waiting));

        // Lo que está en curso o salió bien no se reintenta.
        assert!(!Run::Waiting.can_retry());
        assert!(!done(Some(0), true).can_retry());
    }
}
