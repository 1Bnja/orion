//! Los paneles de la pestaña Grafo: a la izquierda las refs del repo (ramas, tags, stashes y
//! worktrees) y a la derecha el detalle del commit elegido, o del WIP, con el diff de un archivo.

use super::chrome::RADIUS;
use super::graph::{GraphView, ROW, date_utc};
use crate::colors::theme;
use crate::git_graph::{self, Commit, Details, FileChange, RefItem, Refs};
use eframe::egui::{self, RichText, Sense, pos2, vec2};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

/// En el caché de diffs, el "commit" de los cambios sin commitear (ningún hash es vacío).
const WIP: &str = "";

/// Cómo se pinta una línea de un diff.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LineKind {
    Added,
    Removed,
    Hunk,
    Plain,
}

/// Clasifica una línea de `git diff`. Las cabeceras `+++ `/`--- ` van sin color.
/// ponytail: mira solo la línea, así que una línea quitada que empiece con `-- ` (o una
/// agregada con `++ `) se lee como cabecera; si molesta, recordar si ya pasó el primer `@@`.
fn line_kind(line: &str) -> LineKind {
    if line.starts_with("+++ ") || line.starts_with("--- ") {
        LineKind::Plain
    } else if line.starts_with('+') {
        LineKind::Added
    } else if line.starts_with('-') {
        LineKind::Removed
    } else if line.starts_with("@@") {
        LineKind::Hunk
    } else {
        LineKind::Plain
    }
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(8)]
}

/// Lo que devuelve un hilo de git.
enum Reply {
    Details(String, Result<Details, String>),
    Diff((String, String), Result<String, String>),
}

/// El estado de los dos paneles. Detalles y diffs se piden a git en hilos y se guardan:
/// un commit no cambia, así que no se vuelven a pedir (los del WIP sí, al recargar).
pub(super) struct Panels {
    repo: PathBuf,
    pub(super) refs: Refs,
    wip: Vec<FileChange>,
    /// El panel derecho muestra el WIP (mientras no haya un commit elegido).
    wip_open: bool,
    /// Aviso del último clic (ej. el commit no está cargado).
    notice: Option<String>,
    /// El archivo cuyo diff se ve: (commit o `WIP`, ruta).
    file: Option<(String, String)>,
    /// `None` = pedido y en camino.
    details: HashMap<String, Option<Result<Details, String>>>,
    diffs: HashMap<(String, String), Option<Result<String, String>>>,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
}

impl Panels {
    pub(super) fn new(repo: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            repo,
            refs: Refs::default(),
            wip: Vec::new(),
            wip_open: false,
            notice: None,
            file: None,
            details: HashMap::new(),
            diffs: HashMap::new(),
            tx,
            rx,
        }
    }

    /// Cambia el WIP por uno recién leído: sus diffs guardados ya no valen.
    pub(super) fn set_wip(&mut self, wip: Vec<FileChange>) {
        self.diffs.retain(|(id, _), _| id != WIP);
        self.wip_open &= !wip.is_empty();
        self.wip = wip;
    }

    /// Recoge lo que trajeron los hilos.
    pub(super) fn poll(&mut self) {
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Details(id, details) => {
                    self.details.insert(id, Some(details));
                }
                // Si se borró mientras tanto (el WIP cambió), la respuesta ya es vieja.
                Reply::Diff(key, diff) => {
                    if let Some(slot) = self.diffs.get_mut(&key) {
                        *slot = Some(diff);
                    }
                }
            }
        }
    }

    fn fetch_details(&mut self, ctx: &egui::Context, id: &str) {
        if self.details.contains_key(id) {
            return;
        }
        self.details.insert(id.to_string(), None);
        let (tx, ctx, repo, id) = (
            self.tx.clone(),
            ctx.clone(),
            self.repo.clone(),
            id.to_string(),
        );
        std::thread::spawn(move || {
            let details = git_graph::details(&repo, &id);
            let _ = tx.send(Reply::Details(id, details));
            ctx.request_repaint();
        });
    }

    fn fetch_diff(&mut self, ctx: &egui::Context, key: &(String, String)) {
        if self.diffs.contains_key(key) {
            return;
        }
        self.diffs.insert(key.clone(), None);
        let (tx, ctx, repo, key) = (self.tx.clone(), ctx.clone(), self.repo.clone(), key.clone());
        std::thread::spawn(move || {
            let (id, path) = &key;
            let diff = if id == WIP {
                let path = format!(":(top,literal){path}");
                let args = ["diff", "HEAD", "--no-color", "--no-ext-diff", "--", &path];
                crate::agents::git(&repo, &args)
            } else {
                git_graph::diff(&repo, id, path)
            };
            let _ = tx.send(Reply::Diff(key, diff));
            ctx.request_repaint();
        });
    }

    /// Elige el commit `hash` y lleva la lista hasta él; si no está cargado, lo avisa.
    fn select(&mut self, view: &mut GraphView, hash: &str) {
        match view.graph.commits.iter().position(|c| c.id == hash) {
            Some(i) => {
                view.selected = Some(i);
                view.scroll_to = Some(i);
                self.notice = None;
            }
            None => {
                self.notice = Some(format!(
                    "El commit {} no está cargado: prueba «Cargar más» o «Todas las ramas».",
                    short(hash)
                ));
            }
        }
    }

    /// El panel de refs. `branch` = la rama filtrada. Devuelve `Some(nueva)` si se pidió
    /// cambiar el filtro (`Some(None)` = todas las ramas).
    pub(super) fn left(
        &mut self,
        ui: &mut egui::Ui,
        view: &mut GraphView,
        branch: Option<&str>,
    ) -> Option<Option<String>> {
        let (mut change, mut clicked) = (None, None);
        egui::Panel::left("grafo-refs")
            .resizable(true)
            .default_size(210.0)
            .size_range(120.0..=420.0)
            .show(ui, |ui| {
                // Un nombre largo se corta con "…" (el completo va en el tooltip).
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                if let Some(branch) = branch {
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("Solo {branch}")).strong());
                    if ui.button("Todas las ramas").clicked() {
                        change = Some(None);
                    }
                    ui.separator();
                }
                let refs = &self.refs;
                let sections: [(&str, &[RefItem], bool); 5] = [
                    ("Locales", &refs.local, true),
                    ("Remotas", &refs.remote, true),
                    ("Tags", &refs.tags, false),
                    ("Stashes", &refs.stashes, false),
                    ("Worktrees", &refs.worktrees, false),
                ];
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        for (title, items, branches) in sections {
                            egui::CollapsingHeader::new(format!("{title} ({})", items.len()))
                                .id_salt(title)
                                .default_open(branches)
                                .show(ui, |ui| {
                                    for item in items {
                                        // De un worktree basta la carpeta; la ruta va en el tooltip.
                                        let name = match title {
                                            "Worktrees" => Path::new(&item.name)
                                                .file_name()
                                                .map_or(item.name.clone(), |n| {
                                                    n.to_string_lossy().into_owned()
                                                }),
                                            _ => item.name.clone(),
                                        };
                                        let mut text = RichText::new(name);
                                        if item.current {
                                            text = text.strong();
                                        }
                                        let hover =
                                            format!("{}\n{}", item.name, short(&item.target));
                                        let response = ui
                                            .selectable_label(item.current, text)
                                            .on_hover_text(hover);
                                        if response.clicked() {
                                            clicked = Some(item.target.clone());
                                        }
                                        if branches {
                                            response.context_menu(|ui| {
                                                if ui.button("Solo esta rama").clicked() {
                                                    change = Some(Some(item.name.clone()));
                                                    ui.close();
                                                }
                                            });
                                        }
                                    }
                                    if items.is_empty() {
                                        ui.weak("Vacío");
                                    }
                                });
                        }
                    });
            });
        if let Some(hash) = clicked {
            self.select(view, &hash);
        }
        change
    }

    /// El panel del commit elegido o del WIP; si no hay ninguno, no se dibuja.
    pub(super) fn right(&mut self, ui: &mut egui::Ui, view: &mut GraphView) {
        if view.selected.is_some() {
            self.wip_open = false;
        }
        let commit = view
            .selected
            .and_then(|i| view.graph.commits.get(i))
            .cloned();
        if commit.is_none() && !self.wip_open {
            return;
        }
        let ctx = ui.ctx().clone();
        let id = commit.as_ref().map_or(WIP, |c| c.id.as_str()).to_string();
        // Cambió el commit: el diff abierto era de otro.
        if self.file.as_ref().is_some_and(|(of, _)| *of != id) {
            self.file = None;
        }
        let files = match &commit {
            Some(_) => {
                self.fetch_details(&ctx, &id);
                let details = self.details[&id].as_ref().and_then(|d| d.as_ref().ok());
                details.map(|d| d.files.clone())
            }
            None => Some(self.wip.clone()),
        };
        let mut select = None;
        egui::Panel::right("grafo-detalle")
            .resizable(true)
            .default_size(380.0)
            .size_range(240.0..=900.0)
            .show(ui, |ui| {
                let height = ui.available_height() * if self.file.is_some() { 0.45 } else { 1.0 };
                egui::ScrollArea::vertical()
                    .id_salt("grafo-detalle-info")
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add_space(6.0);
                        match &commit {
                            Some(commit) => select = self.commit_info(ui, commit),
                            None => {
                                ui.strong("Cambios sin commitear");
                            }
                        }
                        ui.add_space(8.0);
                        if let Some(files) = &files {
                            self.file_list(ui, &id, files);
                        }
                    });
                if let Some((_, path)) = self.file.clone() {
                    self.diff_view(ui, &ctx, (id.clone(), path));
                }
            });
        if let Some(hash) = select {
            self.select(view, &hash);
        }
    }

    /// Hash, autor, fecha, mensaje y padres. Devuelve el padre pulsado, si alguno.
    fn commit_info(&self, ui: &mut egui::Ui, commit: &Commit) -> Option<String> {
        let mut select = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&commit.id).monospace());
            if ui.button("Copiar hash").clicked() {
                ui.ctx().copy_text(commit.id.clone());
            }
        });
        ui.weak(format!("{}  ·  {}", commit.author, date_utc(commit.time)));
        ui.add_space(6.0);
        match self.details.get(&commit.id) {
            Some(Some(Ok(details))) => {
                ui.label(&details.message);
            }
            Some(Some(Err(e))) => {
                ui.label(&commit.subject);
                let error = ui.visuals().error_fg_color;
                ui.colored_label(error, format!("No pude leer el detalle: {e}"));
            }
            _ => {
                ui.label(&commit.subject);
                ui.spinner();
            }
        }
        if !commit.parents.is_empty() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.weak(if commit.parents.len() == 1 {
                    "Padre:"
                } else {
                    "Padres:"
                });
                for parent in &commit.parents {
                    if ui.link(RichText::new(short(parent)).monospace()).clicked() {
                        select = Some(parent.clone());
                    }
                }
            });
        }
        select
    }

    /// Los archivos con sus líneas; clic abre (o cierra) su diff.
    fn file_list(&mut self, ui: &mut egui::Ui, id: &str, files: &[FileChange]) {
        if files.is_empty() {
            ui.weak("Sin cambios de archivos.");
            return;
        }
        let (green, red) = (theme().ansi[2], theme().ansi[1]);
        for file in files {
            ui.horizontal(|ui| {
                match (file.added, file.removed) {
                    (Some(added), Some(removed)) => {
                        ui.colored_label(green, format!("+{added}"));
                        ui.colored_label(red, format!("-{removed}"));
                    }
                    // ponytail: en el WIP `None` es casi siempre un archivo sin rastrear; un
                    // binario ya rastreado y editado también sale como "nuevo".
                    _ => {
                        ui.weak(if id == WIP { "nuevo" } else { "bin" });
                    }
                }
                let open = self
                    .file
                    .as_ref()
                    .is_some_and(|(of, path)| of == id && *path == file.path);
                if ui.selectable_label(open, &file.path).clicked() {
                    self.file = (!open).then(|| (id.to_string(), file.path.clone()));
                }
            });
        }
    }

    /// El diff de un archivo, coloreado, en su propia área con scroll.
    fn diff_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, key: (String, String)) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong(&key.1);
            if ui.small_button("Cerrar").clicked() {
                self.file = None;
            }
        });
        self.fetch_diff(ctx, &key);
        match &self.diffs[&key] {
            None => {
                ui.spinner();
            }
            Some(Err(e)) => {
                let error = ui.visuals().error_fg_color;
                ui.colored_label(error, format!("No pude leer el diff: {e}"));
            }
            Some(Ok(text)) if text.is_empty() => {
                ui.weak("Sin diff de texto (binario o sin rastrear).");
            }
            Some(Ok(text)) => {
                let t = theme();
                let height = ui.text_style_height(&egui::TextStyle::Monospace);
                // ponytail: cuenta y salta líneas en cada frame (O(n)); con diffs enormes,
                // guardar los cortes de línea junto al texto.
                egui::ScrollArea::both()
                    .id_salt("grafo-diff")
                    .auto_shrink(false)
                    .show_rows(ui, height, text.lines().count(), |ui, rows| {
                        for line in text.lines().skip(rows.start).take(rows.len()) {
                            let color = match line_kind(line) {
                                LineKind::Added => t.ansi[2],
                                LineKind::Removed => t.ansi[1],
                                LineKind::Hunk => t.muted(),
                                LineKind::Plain => t.fg,
                            };
                            // La fuente monoespaciada no tiene glifo para el tabulador.
                            let line = RichText::new(line.replace('\t', "    "));
                            ui.add(egui::Label::new(line.monospace().color(color)).extend());
                        }
                    });
            }
        }
    }

    /// Encima de la lista: el aviso del último clic y, si hay cambios sin commitear, la fila WIP.
    pub(super) fn header(&mut self, ui: &mut egui::Ui, view: &mut GraphView) {
        if let Some(notice) = &self.notice {
            let mut hide = false;
            ui.horizontal(|ui| {
                ui.colored_label(ui.visuals().warn_fg_color, notice);
                hide = ui.small_button("Ocultar").clicked();
            });
            if hide {
                self.notice = None;
            }
        }
        if self.wip.is_empty() {
            return;
        }
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
        if response.clicked() {
            self.wip_open = true;
            view.selected = None;
            self.notice = None;
        }
        let open = self.wip_open && view.selected.is_none();
        if open {
            ui.painter().rect_filled(rect, RADIUS, theme().selection);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, RADIUS, theme().hover());
        }
        let n = self.wip.len();
        let text = format!(
            "// WIP · {n} {}",
            if n == 1 { "archivo" } else { "archivos" }
        );
        ui.painter().text(
            pos2(rect.min.x + 8.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::monospace(13.0),
            theme().secondary(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::graph::tests::sample;
    use eframe::egui::Shape;

    #[test]
    fn diff_line_kinds() {
        assert_eq!(line_kind("+nueva"), LineKind::Added);
        assert_eq!(line_kind("+"), LineKind::Added);
        assert_eq!(line_kind("-vieja"), LineKind::Removed);
        assert_eq!(line_kind("@@ -1,2 +1,3 @@ fn main"), LineKind::Hunk);
        assert_eq!(line_kind(" contexto"), LineKind::Plain);
        assert_eq!(line_kind(""), LineKind::Plain);
        assert_eq!(line_kind("+++ b/src/main.rs"), LineKind::Plain);
        assert_eq!(line_kind("--- a/src/main.rs"), LineKind::Plain);
        assert_eq!(line_kind("diff --git a/x b/x"), LineKind::Plain);
    }

    #[test]
    fn panels_draw_without_input() {
        let mut view = GraphView::new(sample());
        view.selected = Some(1);
        let id = view.graph.commits[1].id.clone();
        let mut panels = Panels::new(PathBuf::from("."));
        let item = |name: &str, current| RefItem {
            name: name.into(),
            target: id.clone(),
            current,
        };
        panels.refs = Refs {
            local: vec![item("main", true), item("orion/tarea-1", false)],
            remote: vec![item("origin/main", false)],
            tags: vec![item("v0.1", false)],
            stashes: Vec::new(),
            worktrees: vec![item("C:/repos/orion", true)],
        };
        let change = |path: &str, added, removed| FileChange {
            path: path.into(),
            added,
            removed,
        };
        panels.set_wip(vec![
            change("a.txt", Some(1), Some(0)),
            change("b.txt", None, None),
        ]);
        // Detalle y diff ya guardados: no se lanza git.
        let details = Details {
            message: "Merge branch 'orion/tarea-1'\n\ncuerpo".into(),
            files: vec![
                change("src/a.rs", Some(2), Some(1)),
                change("logo.png", None, None),
            ],
        };
        panels.details.insert(id.clone(), Some(Ok(details)));
        let key = (id.clone(), "src/a.rs".to_string());
        let diff = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-viejo\n+nuevo";
        panels.diffs.insert(key.clone(), Some(Ok(diff.into())));
        panels.file = Some(key);

        let mut output = egui::Context::default().run_ui(Default::default(), |ui| {
            assert_eq!(panels.left(ui, &mut view, Some("main")), None);
            panels.right(ui, &mut view);
            panels.header(ui, &mut view);
        });
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for expected in [
            "Todas las ramas",
            "Locales (2)",
            "Copiar hash",
            "+2",
            "bin",
            "-viejo",
            "+nuevo",
            "// WIP · 2 archivos",
        ] {
            assert!(texts.iter().any(|t| t == expected), "{expected}: {texts:?}");
        }
        // Sin clics, nada cambió.
        assert_eq!(view.selected, Some(1));
        assert!(panels.notice.is_none());
        output.textures_delta.clear();
    }
}
