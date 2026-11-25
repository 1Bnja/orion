//! La vista "Grafo" (solo lectura): las ramas y los commits del repo, uno por fila, con las
//! líneas del grafo a la izquierda.

use super::chrome::{RADIUS, label, medium};
use crate::colors::theme;
use crate::git_graph::{self, Commit, Graph, Row};
use eframe::egui::{self, Color32, FontId, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Alto fijo de cada fila (lo necesita `show_rows`).
const ROW: f32 = 28.0;
/// Distancia entre carriles.
const LANE: f32 = 16.0;
/// Columnas de la derecha.
const AUTHOR_W: f32 = 130.0;
const DATE_W: f32 = 84.0;
/// Colores ANSI para los carriles: sin negros, grises ni blancos (0, 7, 8 y 15).
const LANE_COLORS: [usize; 12] = [4, 2, 5, 3, 6, 1, 12, 10, 13, 11, 14, 9];

/// El estado de la vista.
pub(super) struct GraphView {
    pub(super) graph: Graph,
    /// Índice en `graph.commits`. Al recargar el grafo se vuelve a ubicar por id (ver `replace`).
    pub(super) selected: Option<usize>,
    pub(super) filter: String,
    pub(super) show_remotes: bool,
    pub(super) show_tags: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RefKind {
    Head,
    Orion,
    Local,
    Remote,
    Tag,
}

/// Qué es una ref (como las escribe `git log --decorate`) y el texto de su chip.
/// ponytail: el contrato solo trae el nombre, así que son remotas las de `origin/` y
/// `upstream/`; con otros nombres de remoto, `git_graph` tendría que decir cuáles lo son.
fn ref_kind(name: &str) -> (RefKind, &str) {
    if let Some(tag) = name.strip_prefix("tag: ") {
        (RefKind::Tag, tag)
    } else if name == "HEAD" || name.starts_with("HEAD -> ") {
        (RefKind::Head, name)
    } else if name.starts_with("origin/") || name.starts_with("upstream/") {
        (RefKind::Remote, name)
    } else if name.starts_with("orion/") {
        (RefKind::Orion, name)
    } else {
        (RefKind::Local, name)
    }
}

/// ¿El commit coincide con la búsqueda? `needle` ya viene en minúsculas.
fn matches(commit: &Commit, needle: &str) -> bool {
    [&commit.subject, &commit.author, &commit.id]
        .into_iter()
        .chain(&commit.refs)
        .any(|text| text.to_lowercase().contains(needle))
}

/// Fecha relativa a `now`; ambos en segundos desde 1970.
fn ago(now: i64, time: i64) -> String {
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    match now.saturating_sub(time) {
        ..MIN => "ahora".into(), // incluye relojes adelantados
        s @ ..HOUR => format!("hace {} min", s / MIN),
        s @ ..DAY => format!("hace {} h", s / HOUR),
        s if s < 30 * DAY => format!("hace {} d", s / DAY),
        s if s < 365 * DAY => match s / (30 * DAY) {
            1 => "hace 1 mes".into(),
            n => format!("hace {n} meses"),
        },
        s => format!("hace {} a", s / (365 * DAY)),
    }
}

/// Fecha absoluta en UTC (la biblioteca estándar no sabe la zona horaria).
fn date_utc(time: i64) -> String {
    let (days, secs) = (time.div_euclid(86_400), time.rem_euclid(86_400));
    // Días → año, mes y día ("civil_from_days", de Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        secs / 3600,
        secs / 60 % 60
    )
}

fn lane_color(lane: usize) -> Color32 {
    theme().ansi[LANE_COLORS[lane % LANE_COLORS.len()]]
}

/// El grafo de la fila `i`: las líneas que entran desde la fila anterior, las que salen hacia
/// la siguiente y el nodo. Cada línea va de centro a centro y se recorta a la fila, así se ve
/// continua aunque `show_rows` solo pinte las visibles.
fn draw_lanes(painter: &egui::Painter, rect: Rect, rows: &[Row], i: usize, opacity: f32) {
    let mut painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let x = |lane: usize| rect.min.x + (lane as f32 + 0.5) * LANE;
    let y = rect.center().y;
    for (row, top) in [(i.checked_sub(1), y - ROW), (Some(i), y)] {
        let Some(row) = row.and_then(|r| rows.get(r)) else {
            continue;
        };
        for &(from, to) in &row.edges {
            // Una línea que cambia de carril lleva el color del carril lateral.
            let stroke = Stroke::new(1.5, lane_color(from.max(to)));
            painter.line_segment([pos2(x(from), top), pos2(x(to), top + ROW)], stroke);
        }
    }
    if let Some(row) = rows.get(i) {
        painter.multiply_opacity(opacity);
        painter.circle_filled(pos2(x(row.lane), y), 4.5, lane_color(row.lane));
    }
}

/// Un chip de ref que empieza en `x`. Devuelve su ancho.
fn chip(painter: &egui::Painter, x: f32, y: f32, kind: RefKind, text: &str) -> f32 {
    let t = theme();
    let (fill, border, color) = match kind {
        RefKind::Head => (t.accent(), t.accent(), t.bg),
        RefKind::Orion => (t.ansi[5].gamma_multiply(0.22), t.ansi[5], t.fg),
        RefKind::Local => (t.hover(), t.muted(), t.fg),
        RefKind::Remote => (Color32::TRANSPARENT, t.line(), t.muted()),
        RefKind::Tag => (Color32::TRANSPARENT, t.ansi[3], t.ansi[3]),
    };
    let galley = painter.layout_no_wrap(text.to_string(), medium(11.0), color);
    let size = galley.size();
    let rect = Rect::from_min_size(pos2(x, y - 9.0), vec2(size.x + 12.0, 18.0));
    // Las etiquetas (tags) van como píldora para distinguirlas de las ramas.
    let radius = if kind == RefKind::Tag { 9.0 } else { 4.0 };
    painter.rect(
        rect,
        radius,
        fill,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    painter.galley(pos2(x + 6.0, y - size.y / 2.0), galley, color);
    rect.width()
}

impl GraphView {
    pub(super) fn new(graph: Graph) -> Self {
        Self {
            graph,
            selected: None,
            filter: String::new(),
            show_remotes: true,
            show_tags: true,
        }
    }

    /// Cambia el grafo por uno recién leído. La selección sigue al mismo commit (por su id: los
    /// índices cambian si entraron commits nuevos) y se pierde si ese commit ya no está.
    pub(super) fn replace(&mut self, graph: Graph) {
        let id = self.selected.and_then(|i| self.graph.commits.get(i));
        self.selected = id.and_then(|old| graph.commits.iter().position(|c| c.id == old.id));
        self.graph = graph;
    }

    /// Dibuja la vista; `busy` = se está recargando. Devuelve true si se pidió refrescar.
    pub fn ui(&mut self, ui: &mut egui::Ui, busy: bool) -> bool {
        let refresh = self.toolbar(ui, busy);
        ui.separator();
        // El panel de abajo va antes que la lista: así la lista ocupa lo que sobra.
        self.details(ui);
        self.list(ui);
        refresh
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, busy: bool) -> bool {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Buscar por asunto, autor, hash o ref…")
                    .desired_width(280.0),
            );
            ui.add_space(8.0);
            ui.checkbox(&mut self.show_remotes, "Remotas");
            ui.checkbox(&mut self.show_tags, "Tags");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let refresh = ui
                    .add_enabled(!busy, egui::Button::new("Refrescar"))
                    .clicked();
                if busy {
                    ui.spinner();
                }
                refresh
            })
            .inner
        })
        .inner
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        if self.graph.commits.is_empty() {
            ui.weak("No hay commits que mostrar.");
            return;
        }
        // Filtrar atenúa las filas que no coinciden en vez de ocultarlas: el grafo sigue
        // entero y no hay que recalcular carriles.
        // ponytail: en repos grandes hay que desplazarse hasta las coincidencias; si molesta,
        // agregar "siguiente coincidencia".
        let needle = self.filter.trim().to_lowercase();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        // Sin espacio entre filas, para que las líneas no se corten.
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            ROW,
            self.graph.commits.len(),
            |ui, visible| {
                for i in visible {
                    self.row(ui, i, &needle, now);
                }
            },
        );
    }

    fn row(&mut self, ui: &mut egui::Ui, i: usize, needle: &str, now: i64) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
        if response.clicked() {
            self.selected = Some(i);
        }
        let commit = &self.graph.commits[i];
        let selected = self.selected == Some(i);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, selected, &commit.subject)
        });
        if selected {
            ui.painter().rect_filled(rect, RADIUS, theme().selection);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, RADIUS, theme().hover());
        }

        let opacity = if needle.is_empty() || matches(commit, needle) {
            1.0
        } else {
            0.3
        };
        let graph_w = self.graph.lanes as f32 * LANE;
        let lanes = Rect::from_min_size(rect.min, vec2(graph_w, ROW));
        draw_lanes(ui.painter(), lanes, &self.graph.rows, i, opacity);

        let mut painter = ui.painter().clone();
        painter.multiply_opacity(opacity);
        let (y, right) = (rect.center().y, rect.max.x - 8.0);
        let column = |from: f32, to: f32| {
            Rect::from_min_max(pos2(from, rect.min.y), pos2(to.max(from), rect.max.y))
        };
        // En una ventana angosta las columnas de la derecha se achican.
        let author_x = right - DATE_W - AUTHOR_W.min(rect.width() * 0.2);
        let text_end = author_x - 8.0;

        // Chips: recortados para que no pisen al autor.
        let mut x = lanes.max.x + 8.0;
        let chips = painter.with_clip_rect(column(x, text_end).intersect(painter.clip_rect()));
        for name in &commit.refs {
            let (kind, text) = ref_kind(name);
            if (kind == RefKind::Remote && !self.show_remotes)
                || (kind == RefKind::Tag && !self.show_tags)
            {
                continue;
            }
            x += chip(&chips, x, y, kind, text) + 4.0;
        }

        let font = FontId::proportional(13.0);
        let small = FontId::proportional(12.0);
        label(
            &painter,
            column(x, text_end),
            &commit.subject,
            font,
            theme().fg,
        );
        label(
            &painter,
            column(author_x, right - DATE_W - 8.0),
            &commit.author,
            small.clone(),
            theme().secondary(),
        );
        label(
            &painter,
            column(right - DATE_W, right),
            ago(now, commit.time),
            small,
            theme().muted(),
        );
    }

    /// El panel de abajo con los datos del commit elegido.
    fn details(&self, ui: &mut egui::Ui) {
        let Some(commit) = self.selected.and_then(|i| self.graph.commits.get(i)) else {
            return;
        };
        egui::Panel::bottom("grafo-detalle")
            .resizable(false)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.monospace(&commit.id);
                    if ui.button("Copiar hash").clicked() {
                        ui.ctx().copy_text(commit.id.clone());
                    }
                });
                ui.weak(format!("{}  ·  {}", commit.author, date_utc(commit.time)));
                ui.add_space(4.0);
                ui.label(&commit.subject);
                ui.add_space(6.0);
            });
    }
}

/// Cuántos commits se leen: alcanza para la historia reciente y carga rápido.
/// ponytail: tope fijo; si hace falta más historia, cargar más al llegar al final de la lista.
const LIMIT: usize = 2000;

/// Una pestaña "Grafo": el grafo de un repo, leído con git en un hilo aparte.
pub(super) struct GraphTab {
    repo: PathBuf,
    /// None hasta que termina la primera carga.
    view: Option<GraphView>,
    /// Una carga en camino.
    loading: Option<Receiver<Result<Graph, String>>>,
    /// La última carga falló (se sigue mostrando el grafo anterior, si había).
    error: Option<String>,
    /// Era la pestaña activa en el frame anterior (para recargar cuando vuelve a serlo).
    pub(super) shown: bool,
}

impl GraphTab {
    /// Una pestaña vacía: carga el grafo la primera vez que queda activa.
    pub(super) fn new(repo: PathBuf) -> Self {
        Self {
            repo,
            view: None,
            loading: None,
            error: None,
            shown: false,
        }
    }

    /// Vuelve a leer el grafo en un hilo (si ya hay una carga en camino, no hace nada).
    pub(super) fn reload(&mut self, ctx: &egui::Context) {
        if self.loading.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (repo, ctx) = (self.repo.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(git_graph::load(&repo, LIMIT));
            ctx.request_repaint(); // para que `poll` lo vea aunque nada más pase
        });
        self.loading = Some(rx);
    }

    /// Recoge la carga si terminó.
    pub(super) fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("la carga se interrumpió".into()),
        };
        self.loading = None;
        match result {
            Ok(graph) => {
                self.error = None;
                match &mut self.view {
                    Some(view) => view.replace(graph),
                    None => self.view = Some(GraphView::new(graph)),
                }
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// La segunda línea de su fila en la barra lateral.
    pub(super) fn detail(&self) -> String {
        match (&self.view, &self.error) {
            (_, Some(_)) => "No se pudo leer".into(),
            (None, None) => "Cargando…".into(),
            (Some(view), None) => format!("{} commits", view.graph.commits.len()),
        }
    }

    /// Dibuja la pestaña: un spinner hasta la primera carga, después la vista.
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.loading.is_some();
        if let Some(e) = &self.error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("No pude leer el grafo: {e}"),
            );
        }
        let Some(view) = &mut self.view else {
            if busy {
                ui.centered_and_justified(|ui| ui.spinner());
            }
            return;
        };
        if view.ui(ui, busy) {
            self.reload(ui.ctx());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Datos de ejemplo: `main` con una rama lateral que se fusiona.
    fn sample() -> Graph {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let id = |n: usize| format!("{:08x}", 0x9e37_79b9_u32.wrapping_mul(n as u32 + 1)).repeat(5);
        // (padres, refs, horas de antigüedad, asunto, carril, líneas)
        type Entry<'a> = (
            &'a [usize],
            &'a [&'a str],
            i64,
            &'a str,
            usize,
            &'a [(usize, usize)],
        );
        let entries: [Entry; 8] = [
            (
                &[1],
                &["HEAD -> main"],
                0,
                "docs: explica la pestaña Grafo",
                0,
                &[(0, 0)],
            ),
            (
                &[2, 3],
                &["origin/main", "origin/HEAD"],
                3,
                "Merge branch 'orion/tarea-1'",
                0,
                &[(0, 0), (0, 1)],
            ),
            (
                &[5],
                &[],
                5,
                "fix: el panel no pierde el foco",
                0,
                &[(0, 0), (1, 1)],
            ),
            (
                &[4],
                &["orion/tarea-1", "origin/orion/tarea-1"],
                8,
                "feat: filtro por autor",
                1,
                &[(0, 0), (1, 1)],
            ),
            (
                &[5],
                &[],
                26,
                "feat: lista de commits",
                1,
                &[(0, 0), (1, 0)],
            ),
            (&[6], &["tag: v0.1"], 50, "release: v0.1", 0, &[(0, 0)]),
            (&[7], &[], 24 * 40, "feat: lee el log de git", 0, &[(0, 0)]),
            (&[], &[], 24 * 400, "Commit inicial", 0, &[]),
        ];
        let mut graph = Graph {
            commits: Vec::new(),
            rows: Vec::new(),
            lanes: 2,
        };
        for (n, (parents, refs, hours, subject, lane, edges)) in entries.into_iter().enumerate() {
            graph.commits.push(Commit {
                id: id(n),
                parents: parents.iter().map(|&p| id(p)).collect(),
                refs: refs.iter().map(|r| r.to_string()).collect(),
                author: if lane == 0 { "Ana Pérez" } else { "Claude" }.into(),
                time: now - hours * 3600,
                subject: subject.into(),
            });
            graph.rows.push(Row {
                lane,
                edges: edges.to_vec(),
            });
        }
        graph
    }

    #[test]
    fn relative_dates() {
        let ago = |secs: i64| ago(100_000_000, 100_000_000 - secs);
        assert_eq!(ago(-50), "ahora");
        assert_eq!(ago(59), "ahora");
        assert_eq!(ago(60), "hace 1 min");
        assert_eq!(ago(3 * 3600), "hace 3 h");
        assert_eq!(ago(2 * 86_400), "hace 2 d");
        assert_eq!(ago(30 * 86_400), "hace 1 mes");
        assert_eq!(ago(90 * 86_400), "hace 3 meses");
        assert_eq!(ago(800 * 86_400), "hace 2 a");
    }

    #[test]
    fn absolute_dates() {
        assert_eq!(date_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(date_utc(951_827_696), "2000-02-29 12:34 UTC");
        assert_eq!(date_utc(-1), "1969-12-31 23:59 UTC");
    }

    #[test]
    fn search_and_ref_kinds() {
        let commit = &sample().commits[3];
        for needle in ["filtro", "claude", &commit.id[..7], "orion/tarea"] {
            assert!(matches(commit, needle), "{needle}");
        }
        assert!(!matches(commit, "nada"));
        assert_eq!(ref_kind("HEAD -> main").0, RefKind::Head);
        assert_eq!(ref_kind("orion/tarea-1").0, RefKind::Orion);
        assert_eq!(ref_kind("origin/orion/tarea-1").0, RefKind::Remote);
        assert_eq!(ref_kind("feature/x").0, RefKind::Local);
        assert_eq!(ref_kind("tag: v0.1"), (RefKind::Tag, "v0.1"));
    }

    #[test]
    fn view_draws_without_input() {
        let mut view = GraphView::new(sample());
        view.selected = Some(1);
        view.filter = "merge".into();
        let mut refresh = true;
        let mut output = egui::Context::default().run_ui(Default::default(), |ui| {
            refresh = view.ui(ui, false);
        });
        assert!(!refresh);
        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }

    #[test]
    fn reloading_keeps_the_selection_by_id() {
        let mut view = GraphView::new(sample());
        view.selected = Some(3);
        let id = view.graph.commits[3].id.clone();
        // Entró un commit nuevo arriba: el elegido baja una fila.
        let mut newer = sample();
        let mut top = newer.commits[0].clone();
        top.id = "nuevo".into();
        newer.commits.insert(0, top);
        newer.rows.insert(0, newer.rows[0].clone());
        view.replace(newer);
        assert_eq!(view.selected, Some(4));
        assert_eq!(view.graph.commits[4].id, id);
        // El commit ya no está (ej. se reescribió la rama): no queda nada elegido.
        let mut gone = sample();
        gone.commits.remove(3);
        gone.rows.remove(3);
        view.replace(gone);
        assert_eq!(view.selected, None);
    }
}
