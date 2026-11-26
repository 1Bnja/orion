//! Las acciones de git de la pestaña Grafo: la barra de arriba, el panel de commit (WIP), los
//! menús de clic derecho, la franja de conflictos y el formulario de PR. Todo git corre en un
//! hilo, una operación a la vez; al terminar, la pestaña recarga el grafo.

use super::chrome::{
    Icon, RADIUS, draw_icon, icon_button, icon_text_button, label, medium, selectable_row, toggle,
};
use super::graph_panels::change_icon;
use crate::colors::theme;
use crate::git_graph::{FileChange, Refs};
use crate::git_ops::{self, InProgress, PullMode, ResetMode, WorkStatus};
use crate::links::{self, Link};
use eframe::egui::{self, Color32, FontId, Key, Modifiers, RichText, Sense, pos2, vec2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// Lo que se lee del repo junto con el grafo, en el hilo de carga.
#[derive(Default)]
pub(super) struct State {
    status: WorkStatus,
    tracking: HashMap<String, (u32, u32)>,
    /// Asunto del último commit: el título por defecto de un PR.
    subject: String,
}

/// Lee el estado del repo. Lo que falle queda vacío: el grafo igual sirve.
pub(super) fn load(repo: &Path) -> State {
    State {
        status: git_ops::status(repo).unwrap_or_default(),
        tracking: git_ops::tracking(repo).unwrap_or_default(),
        subject: crate::agents::git(repo, &["log", "-1", "--format=%s"]).unwrap_or_default(),
    }
}

/// Lo que se pulsó con clic derecho en el panel de refs o en un chip.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Target {
    Local(String),
    Remote(String),
    Tag(String),
    /// El nombre (`stash@{0}: …`) y el commit al que apuntaba cuando se pulsó.
    Stash(String, String),
    Worktree(PathBuf),
}

/// El índice de un stash a partir de su nombre (`stash@{2}: On main: …`).
fn stash_index(name: &str) -> Option<usize> {
    name.strip_prefix("stash@{")?
        .split_once('}')?
        .0
        .parse()
        .ok()
}

/// `take` recibe "ours" como lo entiende git. Durante un rebase git los invierte (ours = la
/// rama de destino, theirs = el commit propio que se aplica), así que "el mío" es theirs.
fn take_ours(op: Option<InProgress>, mine: bool) -> bool {
    if op == Some(InProgress::Rebase) {
        !mine
    } else {
        mine
    }
}

/// El color del contador del resumen: gris hasta 50, amarillo hasta 72, rojo después.
fn summary_color(chars: usize) -> Color32 {
    match chars {
        ..=50 => theme().muted(),
        51..=72 => theme().ansi[3],
        _ => theme().ansi[1],
    }
}

fn op_name(op: InProgress) -> &'static str {
    match op {
        InProgress::Merge => "Merge",
        InProgress::Rebase => "Rebase",
        InProgress::CherryPick => "Cherry-pick",
        InProgress::Revert => "Revert",
    }
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(8)]
}

/// Lo que devuelve el hilo de una operación.
enum Done {
    Text(Result<String, String>),
    /// El commit y, si se pidió, el push que lo siguió.
    Commit(Result<String, String>, Option<Result<String, String>>),
    /// El mensaje del último commit, para Amend.
    Message(Result<String, String>),
    /// Borrar una rama sin forzar: si falla, se ofrece forzar.
    DeleteBranch(String, Result<String, String>),
    /// La URL del PR, o el error y la página para abrirlo a mano.
    Pr(Result<String, (String, Option<String>)>),
}

type Job = Box<dyn FnOnce(&Path) -> Done + Send>;

fn text(f: impl FnOnce(&Path) -> Result<String, String> + Send + 'static) -> Job {
    Box::new(move |repo| Done::Text(f(repo)))
}

/// Un nombre que pide un diálogo.
#[derive(Clone)]
enum NameKind {
    /// Rama nueva en ese commit (o `HEAD`).
    Branch(String),
    Tag(String),
    /// Renombrar esa rama.
    Rename(String),
}

struct PrForm {
    title: String,
    body: String,
    base: String,
    head: String,
    link: Option<String>,
    error: Option<(String, Option<String>)>,
}

enum Dialog {
    /// Una operación que hay que confirmar. `job` se saca al confirmar.
    Confirm {
        title: String,
        text: String,
        button: &'static str,
        job: Option<Job>,
    },
    Name {
        kind: NameKind,
        value: String,
        checkout: bool,
    },
    Pr(PrForm),
}

fn confirm(
    title: impl Into<String>,
    text: impl Into<String>,
    button: &'static str,
    job: Job,
) -> Dialog {
    Dialog::Confirm {
        title: title.into(),
        text: text.into(),
        button,
        job: Some(job),
    }
}

/// Un ítem de menú; si se pulsó (y está habilitado), cierra el menú.
fn item(ui: &mut egui::Ui, icon: Icon, text: &str, enabled: bool) -> bool {
    let clicked = icon_text_button(ui, icon, text, enabled).clicked() && enabled;
    if clicked {
        ui.close();
    }
    clicked
}

/// Título de una sección del panel de commit, con su cuenta y un botón a la derecha.
fn section(ui: &mut egui::Ui, title: &str, count: usize, button: Option<(Icon, &str)>) -> bool {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let pressed = button.is_some_and(|(icon, text)| {
                count > 0 && icon_text_button(ui, icon, text, count > 0).clicked()
            });
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
            let used = label(ui.painter(), rect, title, medium(12.5), theme().fg);
            let rest =
                egui::Rect::from_min_max(pos2(rect.min.x + used + 6.0, rect.min.y), rect.max);
            label(
                ui.painter(),
                rest,
                format!("({count})"),
                FontId::proportional(11.5),
                theme().muted(),
            );
            pressed
        })
        .inner
    })
    .inner
}

/// Una fila de archivo. Con el mouse encima (y `enabled`) muestra `buttons` a la derecha.
/// Devuelve si se pulsó la fila y cuál botón.
fn file_row(
    ui: &mut egui::Ui,
    salt: &str,
    file: &FileChange,
    open: bool,
    buttons: &[(Icon, &str)],
    enabled: bool,
) -> (bool, Option<usize>) {
    let (rect, response) = selectable_row(ui, 24.0, open);
    let y = rect.center().y;
    draw_icon(
        ui,
        change_icon(file, true),
        pos2(rect.min.x + 12.0, y),
        theme().secondary(),
    );
    // Se mira el puntero y no `hovered()`: sobre un botón, la fila deja de estar "hovered".
    let shown = enabled && ui.rect_contains_pointer(rect);
    let end = rect.max.x
        - 6.0
        - if shown {
            24.0 * buttons.len() as f32
        } else {
            0.0
        };
    let path_rect =
        egui::Rect::from_min_max(pos2(rect.min.x + 26.0, rect.min.y), pos2(end, rect.max.y));
    label(
        ui.painter(),
        path_rect,
        &file.path,
        FontId::proportional(12.5),
        theme().fg,
    );
    let mut pressed = None;
    if shown {
        for (k, (icon, tip)) in buttons.iter().enumerate() {
            let center = pos2(rect.max.x - 14.0 - 24.0 * k as f32, y);
            let id = ui.make_persistent_id(("archivo", salt, &file.path, k));
            let button = icon_button(
                ui,
                egui::Rect::from_center_size(center, vec2(22.0, 22.0)),
                id,
                *icon,
            );
            if button.on_hover_text(*tip).clicked() {
                pressed = Some(k);
            }
        }
    }
    let clicked = response.on_hover_text(&file.path).clicked();
    (clicked && pressed.is_none(), pressed)
}

/// Las acciones de una pestaña Grafo.
pub(super) struct Actions {
    repo: PathBuf,
    /// La operación en curso: su nombre y por dónde llega el resultado.
    running: Option<(String, Receiver<Done>)>,
    /// La última operación que falló y su error completo.
    error: Option<(String, String)>,
    state: State,
    /// La rama actual; `None` con HEAD suelto.
    current: Option<String>,
    stashes: usize,
    dialog: Option<Dialog>,
    summary: String,
    description: String,
    amend: bool,
    /// Una carpeta donde abrir una terminal (la recoge `app`).
    pub(super) terminal: Option<PathBuf>,
}

impl Actions {
    pub(super) fn new(repo: PathBuf) -> Self {
        Self {
            repo,
            running: None,
            error: None,
            state: State::default(),
            current: None,
            stashes: 0,
            dialog: None,
            summary: String::new(),
            description: String::new(),
            amend: false,
            terminal: None,
        }
    }

    /// Lo que trajo la última carga.
    pub(super) fn set_state(&mut self, state: State, refs: &Refs) {
        self.current = refs
            .local
            .iter()
            .find(|r| r.current)
            .map(|r| r.name.clone());
        self.stashes = refs.stashes.len();
        self.state = state;
    }

    /// (adelante, atrás) de una rama local respecto a su upstream.
    pub(super) fn tracking(&self, branch: &str) -> Option<(u32, u32)> {
        self.state.tracking.get(branch).copied()
    }

    fn idle(&self) -> bool {
        self.running.is_none()
    }

    /// Corre `job` en un hilo. Si ya hay una operación en curso, no hace nada.
    fn start(&mut self, ctx: &egui::Context, label: impl Into<String>, job: Job) {
        if self.running.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (repo, ctx) = (self.repo.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job(&repo));
            ctx.request_repaint();
        });
        self.error = None;
        self.running = Some((label.into(), rx));
    }

    fn run(
        &mut self,
        ctx: &egui::Context,
        label: impl Into<String>,
        f: impl FnOnce(&Path) -> Result<String, String> + Send + 'static,
    ) {
        self.start(ctx, label, text(f));
    }

    /// Recoge la operación que terminó. Devuelve true si hay que recargar el grafo.
    pub(super) fn poll(&mut self) -> bool {
        let Some((label, rx)) = &self.running else {
            return false;
        };
        let done = match rx.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                Done::Text(Err("la operación se interrumpió".into()))
            }
        };
        let label = label.clone();
        self.running = None;
        let mut failed = None;
        match done {
            Done::Text(result) => failed = result.err(),
            Done::Commit(commit, push) => match commit {
                Ok(_) => {
                    self.summary.clear();
                    self.description.clear();
                    self.amend = false;
                    if let Some(Err(e)) = push {
                        failed = Some(format!("El commit quedó hecho, pero el push falló:\n{e}"));
                    }
                }
                Err(e) => failed = Some(e),
            },
            Done::Message(Ok(message)) => {
                let (summary, description) = message.split_once('\n').unwrap_or((&message, ""));
                self.summary = summary.trim().to_string();
                self.description = description.trim().to_string();
            }
            Done::Message(Err(e)) => {
                self.amend = false;
                failed = Some(e);
            }
            Done::DeleteBranch(_, Ok(_)) => {}
            Done::DeleteBranch(name, Err(e)) => {
                let body = format!(
                    "{e}\n\nSi la borras igual, se pierden los commits que no estén en otra rama."
                );
                let title = format!("Forzar el borrado de {name}");
                let job = text(move |r| git_ops::delete_branch(r, &name, true));
                self.dialog = Some(confirm(title, body, "Borrar igual", job));
            }
            Done::Pr(result) => match &mut self.dialog {
                Some(Dialog::Pr(form)) => match result {
                    Ok(url) => form.link = Some(url),
                    Err(e) => form.error = Some(e),
                },
                _ => failed = result.err().map(|(e, _)| e),
            },
        }
        if let Some(e) = failed {
            self.error = Some((label, e));
        }
        true
    }

    /// La barra de acciones. `selected` = el commit elegido (para crear una rama ahí).
    pub(super) fn bar(&mut self, ui: &mut egui::Ui, selected: Option<&str>) {
        let ctx = ui.ctx().clone();
        let idle = self.idle();
        let branch = self.current.clone();
        let upstream = branch
            .as_ref()
            .is_some_and(|b| self.state.tracking.contains_key(b));
        let status = &self.state.status;
        let dirty = !status.staged.is_empty() || !status.unstaged.is_empty();
        ui.horizontal(|ui| {
            if icon_text_button(ui, Icon::Refresh, "Fetch", idle).clicked() && idle {
                self.run(&ctx, "Fetch", git_ops::fetch);
            }

            let pull = icon_text_button(ui, Icon::ArrowBarToDown, "Pull", idle);
            if idle {
                egui::Popup::menu(&pull).show(|ui| {
                    if item(ui, Icon::ArrowBarToDown, "Pull (solo fast-forward)", true) {
                        self.run(&ctx, "Pull", |r| git_ops::pull(r, PullMode::FastForward));
                    }
                    if item(ui, Icon::GitMerge, "Pull con rebase", true) {
                        self.run(&ctx, "Pull con rebase", |r| {
                            git_ops::pull(r, PullMode::Rebase)
                        });
                    }
                });
            }

            let push = icon_text_button(ui, Icon::ArrowBarUp, "Push", idle && branch.is_some());
            if let (true, Some(b)) = (idle, &branch) {
                egui::Popup::menu(&push).show(|ui| {
                    if !upstream {
                        ui.weak(format!(
                            "{b} no tiene upstream: se sube a origin y queda configurado."
                        ));
                    }
                    if item(ui, Icon::ArrowBarUp, &format!("Push de {b}"), true) {
                        let b = b.clone();
                        self.run(&ctx, "Push", move |r| git_ops::push(r, &b, false));
                    }
                    if item(ui, Icon::AlertTriangle, "Push con force-with-lease…", true) {
                        let name = b.clone();
                        let job = text(move |r| git_ops::push(r, &name, true));
                        self.dialog = Some(confirm(
                            format!("Push forzado de {b}"),
                            "Reemplaza la rama del remoto por la tuya. Si alguien subió commits \
                             que no tienes, git lo rechaza (force-with-lease); si no, lo que \
                             había en el remoto y no está en tu rama se pierde.",
                            "Push forzado",
                            job,
                        ));
                    }
                });
            }
            if branch.is_some() && !upstream {
                let (rect, _) = ui.allocate_exact_size(vec2(80.0, 24.0), Sense::hover());
                label(
                    ui.painter(),
                    rect,
                    "sin upstream",
                    FontId::proportional(11.5),
                    theme().ansi[3],
                );
            }

            if icon_text_button(ui, Icon::GitBranch, "Rama", idle).clicked() && idle {
                let at = selected.unwrap_or("HEAD").to_string();
                self.dialog = Some(Dialog::Name {
                    kind: NameKind::Branch(at),
                    value: String::new(),
                    checkout: true,
                });
            }
            if icon_text_button(ui, Icon::Archive, "Stash", idle && dirty).clicked()
                && idle
                && dirty
            {
                self.run(&ctx, "Stash", |r| git_ops::stash_push(r, None));
            }
            let pop = idle && self.stashes > 0;
            if icon_text_button(ui, Icon::PackageExport, "Pop", pop).clicked() && pop {
                self.run(&ctx, "Pop", |r| git_ops::stash_pop(r, 0));
            }
            let pr = branch.is_some();
            if icon_text_button(ui, Icon::GitPullRequest, "Crear PR", pr).clicked() && pr {
                self.open_pr();
            }
            if let Some((label_text, _)) = &self.running {
                ui.spinner();
                let (rect, _) = ui.allocate_exact_size(vec2(160.0, 24.0), Sense::hover());
                label(
                    ui.painter(),
                    rect,
                    format!("{label_text}…"),
                    FontId::proportional(12.0),
                    theme().muted(),
                );
            }
        });
    }

    fn open_pr(&mut self) {
        if let Some(head) = &self.current {
            self.dialog = Some(Dialog::Pr(PrForm {
                title: self.state.subject.clone(),
                body: String::new(),
                base: "main".into(),
                head: head.clone(),
                link: None,
                error: None,
            }));
        }
    }

    /// El error de la última operación, entero, con un botón para cerrarlo.
    pub(super) fn messages(&mut self, ui: &mut egui::Ui) {
        let Some((what, error)) = &self.error else {
            return;
        };
        let red = theme().ansi[1];
        let mut close = false;
        egui::Frame::new()
            .fill(red.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, red.gamma_multiply(0.6)))
            .corner_radius(RADIUS)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
                draw_icon(
                    ui,
                    Icon::AlertTriangle,
                    pos2(rect.min.x + 9.0, rect.center().y),
                    red,
                );
                let close_rect = egui::Rect::from_center_size(
                    pos2(rect.max.x - 11.0, rect.center().y),
                    vec2(22.0, 22.0),
                );
                let title = egui::Rect::from_min_max(
                    pos2(rect.min.x + 24.0, rect.min.y),
                    pos2(close_rect.min.x - 4.0, rect.max.y),
                );
                label(
                    ui.painter(),
                    title,
                    format!("{what} falló"),
                    medium(12.5),
                    theme().fg,
                );
                let id = ui.make_persistent_id("cerrar-error-git");
                close = icon_button(ui, close_rect, id, Icon::Close)
                    .on_hover_text("Cerrar")
                    .clicked();
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        let text = RichText::new(error).monospace().color(theme().fg);
                        ui.add(egui::Label::new(text).wrap().selectable(true));
                    });
            });
        if close {
            self.error = None;
        }
    }

    /// Sobre la lista, si hay un merge, rebase, cherry-pick o revert a medias.
    pub(super) fn conflict_band(&mut self, ui: &mut egui::Ui) {
        let Some(op) = self.state.status.in_progress else {
            return;
        };
        let ctx = ui.ctx().clone();
        let idle = self.idle();
        let n = self.state.status.conflicted.len();
        let yellow = theme().ansi[3];
        egui::Frame::new()
            .fill(yellow.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, yellow.gamma_multiply(0.6)))
            .corner_radius(RADIUS)
            .inner_margin(egui::Margin::symmetric(8, 2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_text_button(ui, Icon::Check, "Continuar", idle).clicked() && idle {
                            self.run(
                                &ctx,
                                format!("Continuar el {}", op_name(op).to_lowercase()),
                                move |r| git_ops::continue_op(r, op),
                            );
                        }
                        if icon_text_button(ui, Icon::Close, "Abortar", idle).clicked() && idle {
                            let name = op_name(op).to_lowercase();
                            self.dialog = Some(confirm(
                                format!("Abortar el {name}"),
                                "Todo vuelve a como estaba antes de empezar: se pierden los                                  conflictos que ya resolviste. No se puede deshacer.",
                                "Abortar",
                                text(move |r| git_ops::abort(r, op)),
                            ));
                        }
                        let (rect, _) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                        draw_icon(
                            ui,
                            Icon::AlertTriangle,
                            pos2(rect.min.x + 9.0, rect.center().y),
                            yellow,
                        );
                        let text = format!(
                            "{} en curso · {n} {}",
                            op_name(op),
                            if n == 1 { "conflicto" } else { "conflictos" }
                        );
                        let rect =
                            egui::Rect::from_min_max(pos2(rect.min.x + 24.0, rect.min.y), rect.max);
                        label(ui.painter(), rect, text, medium(12.5), theme().fg);
                    });
                });
            });
    }

    /// El panel derecho del WIP: conflictos, sin preparar, preparados y el mensaje.
    /// `open` = el archivo cuyo diff se ve. Devuelve el archivo pulsado.
    pub(super) fn commit_panel(&mut self, ui: &mut egui::Ui, open: Option<&str>) -> Option<String> {
        let ctx = ui.ctx().clone();
        let idle = self.idle();
        // Antes de los campos de texto, para que el Enter no entre como salto de línea.
        let enter = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
        let status = self.state.status.clone();
        let mut clicked = None;

        if !status.conflicted.is_empty() {
            section(ui, "En conflicto", status.conflicted.len(), None);
            for path in &status.conflicted {
                let file = FileChange {
                    path: path.clone(),
                    ..Default::default()
                };
                if file_row(ui, "conflicto", &file, open == Some(path), &[], false).0 {
                    clicked = Some(path.clone());
                }
                // Con `wrapped`, si el panel es angosto los botones bajan de línea en vez de
                // ensanchar el panel.
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(20.0);
                    let mine = icon_text_button(ui, Icon::Check, "Usar el mío", idle).clicked();
                    let theirs =
                        icon_text_button(ui, Icon::GitMerge, "Usar el de ellos", idle).clicked();
                    if idle && (mine || theirs) {
                        let ours = take_ours(status.in_progress, mine);
                        let path = path.clone();
                        self.run(&ctx, format!("Resolver {path}"), move |r| {
                            git_ops::take(r, &path, ours)
                        });
                    }
                    if icon_text_button(ui, Icon::ExternalLink, "Abrir en editor", true).clicked() {
                        links::open(Link::File {
                            path: self.repo.join(path),
                            line: None,
                            col: None,
                        });
                    }
                });
            }
            ui.add_space(8.0);
        }

        let all = |files: &[FileChange]| files.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
        if section(
            ui,
            "Sin preparar",
            status.unstaged.len(),
            Some((Icon::Plus, "Preparar todo")),
        ) && idle
        {
            let paths = all(&status.unstaged);
            self.run(&ctx, "Preparar todo", move |r| git_ops::stage(r, &paths));
        }
        let buttons = [(Icon::Plus, "Preparar"), (Icon::Trash, "Descartar…")];
        for file in &status.unstaged {
            let (row, button) = file_row(
                ui,
                "sin-preparar",
                file,
                open == Some(&file.path),
                &buttons,
                idle,
            );
            let paths = vec![file.path.clone()];
            match button {
                Some(0) => self.run(&ctx, format!("Preparar {}", file.path), move |r| {
                    git_ops::stage(r, &paths)
                }),
                Some(_) => {
                    let job = text(move |r| git_ops::discard(r, &paths));
                    self.dialog = Some(confirm(
                        format!("Descartar {}", file.path),
                        "Se pierden los cambios sin preparar de este archivo (si es nuevo, se \
                         borra). No se puede deshacer.",
                        "Descartar",
                        job,
                    ));
                }
                None if row => clicked = Some(file.path.clone()),
                None => {}
            }
        }
        ui.add_space(8.0);
        if section(
            ui,
            "Preparados",
            status.staged.len(),
            Some((Icon::Close, "Quitar todo")),
        ) && idle
        {
            let paths = all(&status.staged);
            self.run(&ctx, "Quitar todo", move |r| git_ops::unstage(r, &paths));
        }
        for file in &status.staged {
            let (row, button) = file_row(
                ui,
                "preparado",
                file,
                open == Some(&file.path),
                &[(Icon::Close, "Quitar")],
                idle,
            );
            if button.is_some() {
                let paths = vec![file.path.clone()];
                self.run(&ctx, format!("Quitar {}", file.path), move |r| {
                    git_ops::unstage(r, &paths)
                });
            } else if row {
                clicked = Some(file.path.clone());
            }
        }

        ui.add_space(10.0);
        ui.separator();
        let n = self.summary.chars().count();
        ui.horizontal(|ui| {
            let edit = egui::TextEdit::singleline(&mut self.summary)
                .hint_text("Resumen")
                // El contador (28) y el espacio entre los dos.
                .desired_width(
                    (ui.available_width() - 28.0 - ui.spacing().item_spacing.x).max(40.0),
                );
            ui.add(edit);
            let (rect, _) = ui.allocate_exact_size(vec2(28.0, 22.0), Sense::hover());
            label(
                ui.painter(),
                rect,
                n.to_string(),
                FontId::proportional(11.5),
                summary_color(n),
            );
        });
        let description = egui::TextEdit::multiline(&mut self.description)
            .hint_text("Descripción")
            .desired_rows(4)
            .desired_width(f32::INFINITY);
        ui.add(description);
        let was = self.amend;
        toggle(ui, "Amend", &mut self.amend).on_hover_text("Cambia el último commit");
        if self.amend && !was {
            if idle {
                self.start(
                    &ctx,
                    "Leer el último commit",
                    Box::new(|r: &Path| {
                        Done::Message(crate::agents::git(r, &["log", "-1", "--format=%B"]))
                    }),
                );
            } else {
                self.amend = false;
            }
        }
        let ready =
            idle && !self.summary.trim().is_empty() && (!status.staged.is_empty() || self.amend);
        let can_push = ready && self.current.is_some();
        ui.horizontal_wrapped(|ui| {
            let commit = icon_text_button(ui, Icon::GitCommit, "Commit", ready).clicked();
            let push = icon_text_button(ui, Icon::ArrowBarUp, "Commit y push", can_push).clicked();
            ui.weak(keys!("+Enter"));
            if ready && (commit || enter) {
                self.commit(&ctx, false);
            } else if can_push && push {
                self.commit(&ctx, true);
            }
        });
        clicked
    }

    fn commit(&mut self, ctx: &egui::Context, push: bool) {
        let (summary, description) = (self.summary.trim(), self.description.trim());
        let message = if description.is_empty() {
            summary.to_string()
        } else {
            format!("{summary}\n\n{description}")
        };
        let amend = self.amend;
        let branch = self.current.clone().filter(|_| push);
        let label = if push { "Commit y push" } else { "Commit" };
        self.start(
            ctx,
            label,
            Box::new(move |r: &Path| {
                let commit = git_ops::commit(r, &message, amend);
                let push = match (&commit, branch) {
                    (Ok(_), Some(b)) => Some(git_ops::push(r, &b, false)),
                    _ => None,
                };
                Done::Commit(commit, push)
            }),
        );
    }

    /// El menú de clic derecho de un commit.
    pub(super) fn commit_menu(&mut self, ui: &mut egui::Ui, id: &str) {
        let ctx = ui.ctx().clone();
        let idle = self.idle();
        let (id, name) = (id.to_string(), short(id).to_string());
        if item(ui, Icon::Check, "Checkout…", idle) {
            let rev = id.clone();
            self.dialog = Some(confirm(
                format!("Checkout de {name}"),
                "HEAD queda suelto (detached): los commits que hagas ahí no quedan en ninguna \
                 rama hasta que crees una.",
                "Checkout",
                text(move |r| git_ops::checkout(r, &rev)),
            ));
        }
        if item(ui, Icon::GitBranch, "Crear rama aquí…", idle) {
            self.dialog = Some(Dialog::Name {
                kind: NameKind::Branch(id.clone()),
                value: String::new(),
                checkout: true,
            });
        }
        if item(ui, Icon::Tag, "Crear tag aquí…", idle) {
            self.dialog = Some(Dialog::Name {
                kind: NameKind::Tag(id.clone()),
                value: String::new(),
                checkout: false,
            });
        }
        ui.separator();
        if item(ui, Icon::GitCherryPick, "Cherry-pick", idle) {
            let rev = id.clone();
            self.run(&ctx, format!("Cherry-pick de {name}"), move |r| {
                git_ops::cherry_pick(r, &rev)
            });
        }
        if item(ui, Icon::ArrowBackUp, "Revert", idle) {
            let rev = id.clone();
            self.run(&ctx, format!("Revert de {name}"), move |r| {
                git_ops::revert(r, &rev)
            });
        }
        ui.separator();
        let branch = self.current.clone().unwrap_or_else(|| "HEAD".into());
        for (mode, text_mode) in [(ResetMode::Soft, "soft"), (ResetMode::Mixed, "mixed")] {
            if item(
                ui,
                Icon::ArrowBackUp,
                &format!("Reset {text_mode} de {branch} aquí"),
                idle,
            ) {
                let rev = id.clone();
                self.run(&ctx, format!("Reset {text_mode}"), move |r| {
                    git_ops::reset(r, &rev, mode)
                });
            }
        }
        if item(
            ui,
            Icon::AlertTriangle,
            &format!("Reset hard de {branch} aquí…"),
            idle,
        ) {
            let rev = id.clone();
            self.dialog = Some(confirm(
                format!("Reset hard de {branch} a {name}"),
                "Se pierden todos los cambios sin commitear (preparados o no) y los commits de \
                 la rama posteriores a este. No se puede deshacer.",
                "Reset hard",
                text(move |r| git_ops::reset(r, &rev, ResetMode::Hard)),
            ));
        }
        ui.separator();
        if item(ui, Icon::Copy, "Copiar hash", true) {
            ctx.copy_text(id);
        }
    }

    /// Checkout de una rama local (doble clic).
    /// No mira si ya es la rama actual: ese dato puede venir de una carga vieja, y a git le da
    /// lo mismo un checkout a la rama en que ya está.
    pub(super) fn checkout(&mut self, ctx: &egui::Context, branch: &str) {
        if self.idle() {
            let b = branch.to_string();
            self.run(ctx, format!("Checkout de {branch}"), move |r| {
                git_ops::checkout(r, &b)
            });
        }
    }

    /// El menú de clic derecho de una ref.
    pub(super) fn ref_menu(&mut self, ui: &mut egui::Ui, target: &Target) {
        let ctx = ui.ctx().clone();
        let idle = self.idle();
        match target {
            Target::Local(name) => {
                let current = self.current.as_deref() == Some(name.as_str());
                let other = idle && !current && self.current.is_some();
                let into = self.current.clone().unwrap_or_default();
                if item(ui, Icon::Check, "Checkout", idle && !current) {
                    self.checkout(&ctx, name);
                }
                if item(ui, Icon::GitMerge, &format!("Fusionar en {into}"), other) {
                    let n = name.clone();
                    self.run(&ctx, format!("Merge de {name}"), move |r| {
                        git_ops::merge(r, &n)
                    });
                }
                if item(
                    ui,
                    Icon::GitBranch,
                    &format!("Rebase de {into} sobre {name}"),
                    other,
                ) {
                    let n = name.clone();
                    self.run(&ctx, format!("Rebase sobre {name}"), move |r| {
                        git_ops::rebase(r, &n)
                    });
                }
                ui.separator();
                if item(ui, Icon::ArrowBarUp, "Push", idle) {
                    let n = name.clone();
                    self.run(&ctx, format!("Push de {name}"), move |r| {
                        git_ops::push(r, &n, false)
                    });
                }
                let pr = if current {
                    "Crear PR…"
                } else {
                    "Crear PR (solo de la rama actual)"
                };
                if item(ui, Icon::GitPullRequest, pr, current) {
                    self.open_pr();
                }
                ui.separator();
                if item(ui, Icon::Font, "Renombrar…", idle) {
                    self.dialog = Some(Dialog::Name {
                        kind: NameKind::Rename(name.clone()),
                        value: name.clone(),
                        checkout: false,
                    });
                }
                if item(ui, Icon::Trash, "Borrar…", idle && !current) {
                    let n = name.clone();
                    self.dialog = Some(confirm(
                        format!("Borrar la rama {name}"),
                        "Se borra la rama local (la del remoto queda).",
                        "Borrar",
                        Box::new(move |r: &Path| {
                            let result = git_ops::delete_branch(r, &n, false);
                            Done::DeleteBranch(n, result)
                        }),
                    ));
                }
            }
            Target::Remote(name) => {
                if item(
                    ui,
                    Icon::GitBranch,
                    "Crear la rama local que la sigue",
                    idle,
                ) {
                    let n = name.clone();
                    self.run(&ctx, format!("Seguir {name}"), move |r| {
                        git_ops::track(r, &n)
                    });
                }
                if item(ui, Icon::Trash, "Borrar la rama remota…", idle) {
                    let n = name.clone();
                    self.dialog = Some(confirm(
                        format!("Borrar {name} del remoto"),
                        "Se borra la rama en el servidor, para todos.",
                        "Borrar",
                        text(move |r| git_ops::delete_remote_branch(r, &n)),
                    ));
                }
            }
            Target::Tag(name) => {
                if item(ui, Icon::Trash, "Borrar tag", idle) {
                    let n = name.clone();
                    self.run(&ctx, format!("Borrar el tag {name}"), move |r| {
                        git_ops::delete_tag(r, &n)
                    });
                }
            }
            Target::Stash(stash, hash) => {
                let Some(i) = stash_index(stash) else {
                    return;
                };
                // Antes de tocarlo se comprueba que ese número siga siendo el mismo stash.
                let on = |op: fn(&Path, usize) -> Result<String, String>| {
                    let hash = hash.clone();
                    move |r: &Path| git_ops::stash_check(r, i, &hash).and_then(|()| op(r, i))
                };
                if item(ui, Icon::PackageExport, "Aplicar", idle) {
                    self.run(&ctx, "Aplicar el stash", on(git_ops::stash_apply));
                }
                if item(ui, Icon::Archive, "Pop", idle) {
                    self.run(&ctx, "Pop del stash", on(git_ops::stash_pop));
                }
                if item(ui, Icon::Trash, "Borrar…", idle) {
                    self.dialog = Some(confirm(
                        format!("Borrar stash@{{{i}}}"),
                        format!(
                            "Se pierden los cambios guardados en este stash:
{stash}"
                        ),
                        "Borrar",
                        text(on(git_ops::stash_drop)),
                    ));
                }
            }
            Target::Worktree(path) => {
                if item(ui, Icon::Terminal, "Abrir una terminal aquí", true) {
                    self.terminal = Some(path.clone());
                }
            }
        }
    }

    /// El diálogo abierto, si hay uno. Escape lo cierra.
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        let idle = self.idle();
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let mut close = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let mut run: Option<(String, Job)> = None;
        let title = match dialog {
            Dialog::Confirm { title, .. } => title.clone(),
            Dialog::Name {
                kind: NameKind::Branch(_),
                ..
            } => "Rama nueva".into(),
            Dialog::Name {
                kind: NameKind::Tag(_),
                ..
            } => "Tag nuevo".into(),
            Dialog::Name {
                kind: NameKind::Rename(old),
                ..
            } => format!("Renombrar {old}"),
            Dialog::Pr(form) => format!("Pull request de {}", form.head),
        };
        egui::Window::new(title)
            .id(egui::Id::unique("grafo-dialogo"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, vec2(0.0, 48.0))
            .min_width(400.0)
            .max_width(400.0)
            .show(ctx, |ui| match dialog {
                Dialog::Confirm { title, text, button, job } => {
                    ui.horizontal_top(|ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                        draw_icon(ui, Icon::AlertTriangle, rect.center(), theme().ansi[3]);
                        ui.add(egui::Label::new(text.as_str()).wrap());
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if icon_text_button(ui, Icon::Close, "Cancelar", true).clicked() {
                            close = true;
                        }
                        if icon_text_button(ui, Icon::Check, button, idle).clicked() && idle {
                            run = job.take().map(|job| (title.clone(), job));
                            close = true;
                        }
                    });
                }
                Dialog::Name { kind, value, checkout } => {
                    match kind {
                        NameKind::Branch(at) | NameKind::Tag(at) => {
                            ui.weak(format!("En {}", if at == "HEAD" { "HEAD" } else { short(at) }));
                        }
                        NameKind::Rename(_) => {}
                    }
                    let edit = ui.add(
                        egui::TextEdit::singleline(value)
                            .hint_text("Nombre")
                            .desired_width(f32::INFINITY),
                    );
                    edit.request_focus();
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    if let NameKind::Branch(_) = kind {
                        toggle(ui, "Cambiarse a la rama", checkout);
                    }
                    ui.add_space(6.0);
                    let name = value.trim().to_string();
                    let ready = idle && !name.is_empty();
                    ui.horizontal(|ui| {
                        if icon_text_button(ui, Icon::Close, "Cancelar", true).clicked() {
                            close = true;
                        }
                        let ok = if let NameKind::Rename(_) = kind { "Renombrar" } else { "Crear" };
                        if (icon_text_button(ui, Icon::Check, ok, ready).clicked() || enter) && ready {
                            let checkout = *checkout;
                            run = Some(match kind.clone() {
                                NameKind::Branch(at) => (
                                    format!("Crear la rama {name}"),
                                    text(move |r| git_ops::create_branch(r, &name, &at, checkout)),
                                ),
                                NameKind::Tag(at) => (
                                    format!("Crear el tag {name}"),
                                    text(move |r| git_ops::create_tag(r, &name, &at)),
                                ),
                                NameKind::Rename(old) => (
                                    format!("Renombrar {old}"),
                                    text(move |r| git_ops::rename_branch(r, &old, &name)),
                                ),
                            });
                            close = true;
                        }
                    });
                }
                Dialog::Pr(form) => {
                    ui.label("Título");
                    ui.add(egui::TextEdit::singleline(&mut form.title).desired_width(f32::INFINITY));
                    ui.label("Descripción");
                    ui.add(egui::TextEdit::multiline(&mut form.body).desired_rows(5).desired_width(f32::INFINITY));
                    ui.label("Rama base");
                    ui.add(egui::TextEdit::singleline(&mut form.base).desired_width(f32::INFINITY));
                    ui.weak(format!("{} → {}", form.head, form.base.trim()));
                    ui.add_space(6.0);
                    if let Some(link) = &form.link {
                        ui.label("PR creado:");
                        ui.add(egui::Label::new(RichText::new(link).monospace()).selectable(true));
                        if icon_text_button(ui, Icon::ExternalLink, "Abrir en el navegador", true).clicked() {
                            links::open(Link::Url(link.clone()));
                        }
                    }
                    if let Some((error, compare)) = &form.error {
                        let text = RichText::new(error).monospace().color(theme().ansi[1]);
                        ui.add(egui::Label::new(text).wrap().selectable(true));
                        match compare {
                            Some(url) => {
                                ui.weak("Puedes abrirlo a mano en GitHub:");
                                if icon_text_button(ui, Icon::ExternalLink, "Abrir la comparación", true).clicked() {
                                    links::open(Link::Url(url.clone()));
                                }
                            }
                            None => {
                                ui.weak("origin no es un repo de GitHub: no hay página para abrirlo a mano.");
                            }
                        }
                    }
                    ui.add_space(6.0);
                    let ready = idle && form.link.is_none() && !form.title.trim().is_empty() && !form.base.trim().is_empty();
                    ui.horizontal(|ui| {
                        let cancel = if form.link.is_some() { "Cerrar" } else { "Cancelar" };
                        if icon_text_button(ui, Icon::Close, cancel, true).clicked() {
                            close = true;
                        }
                        if icon_text_button(ui, Icon::GitPullRequest, "Crear PR", ready).clicked() && ready {
                            form.error = None;
                            let (title, body) = (form.title.trim().to_string(), form.body.clone());
                            let (base, head) = (form.base.trim().to_string(), form.head.clone());
                            run = Some((
                                "Crear PR".into(),
                                Box::new(move |r: &Path| {
                                    Done::Pr(git_ops::create_pr(r, &title, &body, &base).map_err(|e| {
                                        (e, git_ops::compare_url(r, &base, &head))
                                    }))
                                }),
                            ));
                        }
                        if !idle {
                            ui.spinner();
                        }
                    });
                }
            });
        // El formulario de PR sigue abierto mientras gh trabaja: ahí se muestra el link.
        let keep = matches!(self.dialog, Some(Dialog::Pr(_))) && run.is_some();
        if let Some((label, job)) = run {
            self.start(ctx, label, job);
        }
        if close && !keep {
            self.dialog = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn en_un_rebase_el_mio_es_theirs() {
        // Merge, cherry-pick y revert: "el mío" es ours.
        for op in [
            None,
            Some(InProgress::Merge),
            Some(InProgress::CherryPick),
            Some(InProgress::Revert),
        ] {
            assert!(take_ours(op, true), "{op:?}");
            assert!(!take_ours(op, false), "{op:?}");
        }
        // Rebase: "Usar el mío" → take(…, false); "Usar el de ellos" → take(…, true).
        assert!(!take_ours(Some(InProgress::Rebase), true));
        assert!(take_ours(Some(InProgress::Rebase), false));
    }

    #[test]
    fn indice_de_stash_y_color_del_resumen() {
        assert_eq!(stash_index("stash@{0}: On main: x"), Some(0));
        assert_eq!(stash_index("stash@{12}: WIP on y"), Some(12));
        assert_eq!(stash_index("main"), None);
        assert_eq!(summary_color(50), theme().muted());
        assert_eq!(summary_color(51), theme().ansi[3]);
        assert_eq!(summary_color(72), theme().ansi[3]);
        assert_eq!(summary_color(73), theme().ansi[1]);
    }

    #[test]
    fn el_panel_de_commit_no_ensancha_un_panel_angosto() {
        let mut actions = Actions::new(PathBuf::from("."));
        actions.summary = "un resumen".into();
        actions.state.status = WorkStatus {
            staged: vec![FileChange {
                path: "a.txt".into(),
                ..Default::default()
            }],
            conflicted: vec!["c.txt".into()],
            in_progress: Some(InProgress::Merge),
            ..Default::default()
        };
        let width = 200.0;
        let mut used = 0.0;
        let mut output = egui::Context::default().run_ui(Default::default(), |ui| {
            let rect = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 2000.0));
            let mut narrow = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            actions.commit_panel(&mut narrow, None);
            used = narrow.min_rect().width();
        });
        assert!(used <= width + 0.5, "usó {used} de {width}");
        output.textures_delta.clear();
    }

    #[test]
    fn el_panel_de_commit_se_dibuja_con_conflictos() {
        let mut actions = Actions::new(PathBuf::from("."));
        let change = |path: &str| FileChange {
            path: path.into(),
            added: Some(1),
            removed: Some(0),
        };
        actions.state.status = WorkStatus {
            staged: vec![change("a.txt")],
            unstaged: vec![change("b.txt")],
            conflicted: vec!["c.txt".into()],
            in_progress: Some(InProgress::Rebase),
        };
        actions.error = Some(("Push".into(), "rechazado".into()));
        let mut output = egui::Context::default().run_ui(Default::default(), |ui| {
            actions.bar(ui, None);
            actions.messages(ui);
            actions.conflict_band(ui);
            assert_eq!(actions.commit_panel(ui, None), None);
        });
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for expected in [
            "Rebase en curso · 1 conflicto",
            "Usar el mío",
            "Sin preparar",
            "Preparados",
            "Push falló",
            "rechazado",
        ] {
            assert!(texts.iter().any(|t| t == expected), "{expected}: {texts:?}");
        }
        assert!(actions.idle());
        output.textures_delta.clear();
    }

    /// Maneja la pestaña Grafo de verdad (clics, teclado, menús) con eventos de egui.
    struct Driver {
        ctx: egui::Context,
        tab: crate::app::graph::GraphTab,
        time: f64,
        /// Los textos pintados en el último frame, con su centro.
        texts: Vec<(String, egui::Pos2)>,
    }

    impl Driver {
        fn frame(&mut self, events: Vec<egui::Event>) {
            let screen = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1400.0, 900.0));
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            self.time += 0.05;
            let tab = &mut self.tab;
            let mut output = self.ctx.run_ui(input, |ui| {
                tab.poll(ui.ctx(), true);
                tab.ui(ui);
            });
            self.texts = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => {
                        Some((t.galley.text().to_string(), t.pos + t.galley.size() / 2.0))
                    }
                    _ => None,
                })
                .collect();
            output.textures_delta.clear();
        }

        /// El de más arriba (lo último que se pintó).
        fn find(&self, text: &str) -> Option<egui::Pos2> {
            self.texts
                .iter()
                .rev()
                .find(|(t, _)| t == text)
                .map(|(_, p)| *p)
        }

        fn wait(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
            for _ in 0..600 {
                self.frame(Vec::new());
                if done(self) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let texts: Vec<&str> = self.texts.iter().map(|(t, _)| t.as_str()).collect();
            panic!("no pasó: {what}\ntextos: {texts:?}");
        }

        fn wait_for(&mut self, text: &str) -> egui::Pos2 {
            self.wait(&format!("aparece «{text}»"), |d| d.find(text).is_some());
            self.find(text).unwrap()
        }

        fn wait_gone(&mut self, text: &str) {
            self.wait(&format!("se va «{text}»"), |d| d.find(text).is_none());
        }

        fn press(&mut self, pos: egui::Pos2, button: egui::PointerButton) {
            let event = |pressed| egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            };
            self.frame(vec![event(true)]);
            self.frame(vec![event(false)]);
        }

        fn click_with(&mut self, text: &str, button: egui::PointerButton) {
            let pos = self.wait_for(text);
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            self.press(pos, button);
            // Lejos del próximo clic: que no cuente como doble clic.
            self.time += 1.0;
            self.frame(Vec::new());
        }

        fn click(&mut self, text: &str) {
            self.click_with(text, egui::PointerButton::Primary);
        }

        /// `first`: el primero que se pintó (el del panel izquierdo) y no el de más arriba.
        fn double_click(&mut self, text: &str, first: bool) {
            let mut pos = self.wait_for(text);
            if first {
                pos = self.texts.iter().find(|(t, _)| t == text).unwrap().1;
            }
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            self.press(pos, egui::PointerButton::Primary);
            self.press(pos, egui::PointerButton::Primary);
            self.time += 1.0;
            self.frame(Vec::new());
        }

        fn key(&mut self, key: Key, modifiers: Modifiers) {
            let event = |pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            };
            self.frame(vec![egui::Event::ModifiersChanged(modifiers), event(true)]);
            self.frame(vec![
                event(false),
                egui::Event::ModifiersChanged(Modifiers::NONE),
            ]);
        }

        fn type_text(&mut self, text: &str) {
            self.frame(vec![egui::Event::Text(text.into())]);
        }
    }

    /// El recorrido de la tarea, contra un repo con un remoto bare: commit, push, una rama,
    /// un merge con conflicto y su resolución, todo con clics y teclado en la pestaña.
    #[test]
    fn recorrido_por_la_ui_contra_un_repo_de_verdad() {
        let git = |dir: &Path, args: &[&str]| crate::agents::git(dir, args).unwrap();
        let base = std::env::temp_dir().join(format!("orion-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (repo, bare) = (base.join("repo"), base.join("remoto.git"));
        std::fs::create_dir_all(&repo).unwrap();
        git(&base, &["init", "-q", "--bare", bare.to_str().unwrap()]);
        git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(&repo, &["init", "-q"]);
        git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        for (key, value) in [
            ("user.name", "t"),
            ("user.email", "t@t"),
            ("core.autocrlf", "false"),
            ("commit.gpgsign", "false"),
        ] {
            git(&repo, &["config", key, value]);
        }
        git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
        let write = |file: &str, text: &str| std::fs::write(repo.join(file), text).unwrap();
        write("a.txt", "raiz\n");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "raiz"]);
        git(&repo, &["checkout", "-qb", "lateral"]);
        write("a.txt", "ellos\n");
        git(&repo, &["commit", "-qam", "ellos"]);
        git(&repo, &["checkout", "-q", "main"]);
        write("a.txt", "propio\n");
        write("b.txt", "nuevo\n");

        let mut d = Driver {
            ctx: egui::Context::default(),
            tab: crate::app::graph::GraphTab::new(repo.clone()),
            time: 0.0,
            texts: Vec::new(),
        };

        // Commit: fila WIP, «Preparar todo», el resumen y Ctrl+Enter.
        d.click("// WIP · 2 archivos");
        d.click("Preparar todo");
        d.wait("todo preparado", |_| {
            git(&repo, &["diff", "--name-only"]).is_empty()
        });
        d.wait_for("Quitar todo");
        d.click("Resumen");
        d.type_text("propio");
        d.key(Key::Enter, Modifiers::COMMAND | Modifiers::CTRL);
        d.wait_gone("// WIP · 2 archivos");
        assert_eq!(git(&repo, &["log", "-1", "--format=%s"]), "propio");
        assert!(git(&repo, &["status", "--porcelain"]).is_empty());

        // Push: la rama no tiene upstream y la barra lo dice.
        d.wait_for("sin upstream");
        d.click("Push");
        d.wait_for("main no tiene upstream: se sube a origin y queda configurado.");
        d.click("Push de main");
        d.wait_gone("sin upstream");
        assert_eq!(
            git(&bare, &["rev-parse", "main"]),
            git(&repo, &["rev-parse", "HEAD"])
        );

        // Rama: Escape cancela el diálogo; Enter crea la rama y se cambia a ella.
        d.click("Rama");
        d.wait_for("Rama nueva");
        d.key(Key::Escape, Modifiers::NONE);
        d.wait_gone("Rama nueva");
        d.click("Rama");
        d.wait_for("Rama nueva");
        d.type_text("feature/x");
        d.key(Key::Enter, Modifiers::NONE);
        d.wait("checkout de feature/x", |_| {
            git(&repo, &["branch", "--show-current"]) == "feature/x"
        });
        // Doble clic en una rama del panel izquierdo: checkout.
        d.double_click("main", true);
        d.wait("checkout de main", |_| {
            git(&repo, &["branch", "--show-current"]) == "main"
        });

        // Merge con conflicto desde el menú de la rama: el error entero y la franja.
        d.click_with("lateral", egui::PointerButton::Secondary);
        d.click("Fusionar en main");
        d.wait_for("Merge de lateral falló");
        d.wait_for("Merge en curso · 1 conflicto");
        d.click("// WIP · 1 archivo");
        d.click("Usar el mío");
        d.wait_for("Merge en curso · 0 conflictos");
        d.click("Continuar");
        d.wait_gone("Merge en curso · 0 conflictos");
        let a = std::fs::read_to_string(repo.join("a.txt")).unwrap();
        assert_eq!(a, "propio\n");
        assert_eq!(
            git(&repo, &["rev-list", "--count", "--merges", "HEAD"]),
            "1"
        );
        assert!(git(&repo, &["status", "--porcelain"]).is_empty());

        // Stash y Pop.
        write("a.txt", "a medias\n");
        // La pestaña nota el cambio sola (mira el repo cada pocos segundos).
        d.wait_for("// WIP · 1 archivo");
        d.click("Stash");
        d.wait("stash guardado", |_| {
            git(&repo, &["stash", "list"]).lines().count() == 1
        });
        assert!(git(&repo, &["status", "--porcelain"]).is_empty());
        d.wait_for("refs/stash");
        d.click("Pop");
        d.wait("stash aplicado", |_| {
            git(&repo, &["stash", "list"]).is_empty()
        });
        let a = std::fs::read_to_string(repo.join("a.txt")).unwrap();
        assert_eq!(a, "a medias\n");

        // PR: origin no es de GitHub, así que gh falla, se ve su error y no hay página.
        d.click("Crear PR");
        d.wait_for("Pull request de main");
        d.click("Crear PR");
        d.wait_for("origin no es un repo de GitHub: no hay página para abrirlo a mano.");
        d.key(Key::Escape, Modifiers::NONE);
        d.wait_gone("Pull request de main");

        let _ = std::fs::remove_dir_all(&base);
    }
}
