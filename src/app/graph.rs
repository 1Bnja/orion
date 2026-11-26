//! La vista "Grafo" (solo lectura): las ramas y los commits del repo, uno por fila, con las
//! refs a la izquierda y, junto a ellas, las líneas del grafo.

use super::chrome::{Icon, icon_text_button, label, medium, search_field, selectable_row, toggle};
use super::graph_actions::{self, Actions, Target};
use super::graph_panels::Panels;
use crate::colors::theme;
use crate::git_graph::{self, Commit, FileChange, Graph, Refs, Row};
use eframe::egui::epaint::{CubicBezierShape, RectShape};
use eframe::egui::{self, Color32, FontId, Rect, Sense, Shape, Stroke, StrokeKind, pos2, vec2};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Alto fijo de cada fila (lo necesita `show_rows`).
pub(super) const ROW: f32 = 28.0;
/// Distancia entre carriles.
const LANE: f32 = 24.0;
/// Radio del nodo con las iniciales y del punto de un merge.
const NODE: f32 = 10.0;
const MERGE_NODE: f32 = 4.0;
/// Ancho de la columna de refs, a la izquierda del grafo.
const REFS_W: f32 = 160.0;
/// Límites al arrastrar el borde de la columna de refs.
const REFS_MIN: f32 = 80.0;
const REFS_MAX: f32 = 360.0;
/// Alto de la fila de encabezado de columnas.
const HEADER: f32 = 22.0;
/// Columnas de la derecha.
const DATE_W: f32 = 84.0;
/// Colores ANSI para los carriles: sin negros, grises ni blancos (0, 7, 8 y 15).
const LANE_COLORS: [usize; 12] = [4, 2, 5, 3, 6, 1, 12, 10, 13, 11, 14, 9];
/// Rellenos de los nodos, según el autor: todos se leen con iniciales blancas.
const AUTHOR_COLORS: [Color32; 10] = [
    Color32::from_rgb(0xb9, 0x3c, 0x3c),
    Color32::from_rgb(0xb4, 0x55, 0x09),
    Color32::from_rgb(0x8a, 0x6d, 0x0b),
    Color32::from_rgb(0x3f, 0x7d, 0x20),
    Color32::from_rgb(0x0f, 0x76, 0x6e),
    Color32::from_rgb(0x03, 0x69, 0xa1),
    Color32::from_rgb(0x1d, 0x4e, 0xd8),
    Color32::from_rgb(0x6d, 0x28, 0xd9),
    Color32::from_rgb(0xa2, 0x1c, 0xaf),
    Color32::from_rgb(0xbe, 0x18, 0x5d),
];

/// El estado de la vista.
pub(super) struct GraphView {
    pub(super) graph: Graph,
    /// Índice en `graph.commits`. Al recargar el grafo se vuelve a ubicar por id (ver `replace`).
    pub(super) selected: Option<usize>,
    pub(super) filter: String,
    pub(super) show_remotes: bool,
    pub(super) show_tags: bool,
    /// Fila a la que la lista se desplaza en el próximo frame.
    pub(super) scroll_to: Option<usize>,
    /// Ancho de la columna de refs, que se ajusta arrastrando su borde.
    refs_w: f32,
    /// Carril del commit bajo el puntero en el frame anterior: se resalta en todas las filas.
    hovered_lane: Option<usize>,
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
pub(super) fn date_utc(time: i64) -> String {
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

/// Hasta dos iniciales del autor (primera y última palabra), en mayúscula.
fn initials(author: &str) -> String {
    let mut words = author
        .split_whitespace()
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()));
    let first = words.next();
    let letters: String = first
        .into_iter()
        .chain(words.next_back())
        .flat_map(|c| c.to_uppercase().next())
        .collect();
    if letters.is_empty() {
        "?".into()
    } else {
        letters
    }
}

/// Color estable por autor: FNV-1a del nombre (el hasher de `std` no promete repetir valores).
fn author_color(author: &str) -> Color32 {
    let hash = author
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    AUTHOR_COLORS[(hash % AUTHOR_COLORS.len() as u64) as usize]
}

/// Grosor y opacidad de un carril según cuál está bajo el puntero.
fn emphasis(lane: usize, hovered: Option<usize>) -> (f32, f32) {
    match hovered {
        None => (1.5, 1.0),
        Some(h) if h == lane => (3.0, 1.0),
        Some(_) => (1.5, 0.45),
    }
}

/// Las líneas de la fila `i`: las que entran desde la fila anterior y las que salen hacia la
/// siguiente. Cada línea va de centro a centro y se recorta a la fila, así se ve continua
/// aunque `show_rows` solo pinte las visibles. Las que cambian de carril son curvas.
fn draw_lanes(painter: &egui::Painter, rect: Rect, rows: &[Row], i: usize, hovered: Option<usize>) {
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let x = |lane: usize| rect.min.x + (lane as f32 + 0.5) * LANE;
    let y = rect.center().y;
    for (row, top) in [(i.checked_sub(1), y - ROW), (Some(i), y)] {
        let Some(row) = row.and_then(|r| rows.get(r)) else {
            continue;
        };
        for &(from, to) in &row.edges {
            // Una línea que cambia de carril lleva el color del carril lateral.
            let lane = from.max(to);
            let (width, alpha) = emphasis(lane, hovered);
            let stroke = Stroke::new(width, lane_color(lane).gamma_multiply(alpha));
            let (start, end) = (pos2(x(from), top), pos2(x(to), top + ROW));
            if from == to {
                painter.line_segment([start, end], stroke);
            } else {
                // Sale y llega en vertical, para empalmar con las rectas sin quiebres.
                let middle = top + ROW / 2.0;
                painter.add(CubicBezierShape::from_points_stroke(
                    [start, pos2(start.x, middle), pos2(end.x, middle), end],
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ));
            }
        }
    }
}

/// El nodo de un commit: las iniciales del autor, o un punto chico si es un merge.
fn node(painter: &egui::Painter, center: egui::Pos2, commit: &Commit, color: Color32) {
    if commit.parents.len() > 1 {
        painter.circle_filled(center, MERGE_NODE, color);
        return;
    }
    // El borde (2 px) queda centrado en el radio: el círculo mide `NODE` en total.
    painter.circle(
        center,
        NODE - 1.0,
        author_color(&commit.author),
        Stroke::new(2.0, color),
    );
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        initials(&commit.author),
        medium(9.0),
        Color32::WHITE,
    );
}

/// Un chip de ref con el borde izquierdo en x = 0 y centrado en y = 0 (quien lo llama lo
/// traslada). Devuelve su ancho y sus figuras.
fn chip(painter: &egui::Painter, kind: RefKind, text: &str) -> (f32, [Shape; 2]) {
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
    let rect = Rect::from_min_size(pos2(0.0, -9.0), vec2(size.x + 12.0, 18.0));
    // Las etiquetas (tags) van como píldora para distinguirlas de las ramas.
    let radius = if kind == RefKind::Tag { 9.0 } else { 4.0 };
    let frame = RectShape::new(
        rect,
        radius,
        fill,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    let text = Shape::galley(pos2(6.0, -size.y / 2.0), galley, color);
    (rect.width(), [frame.into(), text])
}

impl GraphView {
    pub(super) fn new(graph: Graph) -> Self {
        Self {
            graph,
            selected: None,
            filter: String::new(),
            show_remotes: true,
            show_tags: true,
            scroll_to: None,
            refs_w: REFS_W,
            hovered_lane: None,
        }
    }

    /// Cambia el grafo por uno recién leído. La selección sigue al mismo commit (por su id: los
    /// índices cambian si entraron commits nuevos) y se pierde si ese commit ya no está.
    pub(super) fn replace(&mut self, graph: Graph) {
        let id = self.selected.and_then(|i| self.graph.commits.get(i));
        self.selected = id.and_then(|old| graph.commits.iter().position(|c| c.id == old.id));
        self.graph = graph;
    }

    /// La barra de arriba; `busy` = se está recargando. Devuelve true si se pidió refrescar.
    fn toolbar(&mut self, ui: &mut egui::Ui, busy: bool) -> bool {
        ui.horizontal(|ui| {
            search_field(
                ui,
                &mut self.filter,
                "Buscar por asunto, autor, hash o ref…",
                280.0,
            );
            ui.add_space(8.0);
            toggle(ui, "Remotas", &mut self.show_remotes);
            toggle(ui, "Tags", &mut self.show_tags);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let refresh =
                    icon_text_button(ui, Icon::Refresh, "Refrescar", !busy).clicked() && !busy;
                if busy {
                    ui.spinner();
                }
                refresh
            })
            .inner
        })
        .inner
    }

    /// La fila de encabezado de columnas y el borde arrastrable de la columna de refs.
    fn header(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER), Sense::hover());
        let graph_x = rect.min.x + self.refs_w;
        let graph_w = self.graph.lanes as f32 * LANE;
        let message_x = graph_x + graph_w + 8.0;
        let right = rect.max.x - 8.0;
        let font = FontId::proportional(10.5);
        let muted = theme().muted();
        let column = |from: f32, to: f32| {
            Rect::from_min_max(pos2(from, rect.min.y), pos2(to.max(from), rect.max.y))
        };
        label(
            ui.painter(),
            column(rect.min.x + 8.0, graph_x - 6.0),
            "RAMA / TAG",
            font.clone(),
            muted,
        );
        label(
            ui.painter(),
            column(graph_x, graph_x + graph_w),
            "GRAFO",
            font.clone(),
            muted,
        );
        label(
            ui.painter(),
            column(message_x, right - DATE_W - 8.0),
            "MENSAJE",
            font.clone(),
            muted,
        );
        label(
            ui.painter(),
            column(right - DATE_W, right),
            "FECHA",
            font,
            muted,
        );
        let line = theme().line().gamma_multiply(0.6);
        ui.painter()
            .hline(rect.x_range(), rect.max.y, Stroke::new(1.0, line));

        let handle = Rect::from_center_size(pos2(graph_x, rect.center().y), vec2(7.0, HEADER));
        let drag = ui.interact(
            handle,
            ui.make_persistent_id("grafo-refs-borde"),
            Sense::drag(),
        );
        if drag.hovered() || drag.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if drag.dragged() {
            self.refs_w = (self.refs_w + drag.drag_delta().x).clamp(REFS_MIN, REFS_MAX);
        }
        let color = if drag.hovered() || drag.dragged() {
            theme().accent()
        } else {
            theme().line()
        };
        ui.painter()
            .vline(graph_x, rect.y_range(), Stroke::new(1.0, color));
    }

    /// La lista de commits. Con `more`, al final va «Cargar más»; devuelve true si se pulsó.
    fn list(&mut self, ui: &mut egui::Ui, more: bool, actions: &mut Actions) -> bool {
        if self.graph.commits.is_empty() {
            ui.weak("No hay commits que mostrar.");
            return false;
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
        self.header(ui);
        // Las filas se pintan con el carril del frame anterior y anotan el de este.
        let hovered = self.hovered_lane.take();
        let mut area = egui::ScrollArea::vertical().auto_shrink(false);
        if let Some(i) = self.scroll_to.take() {
            // La fila queda a media altura (o arriba del todo, si es de las primeras).
            let offset = i as f32 * ROW - (ui.available_height() - ROW) / 2.0;
            area = area.vertical_scroll_offset(offset.max(0.0));
        }
        let count = self.graph.commits.len();
        let mut load_more = false;
        area.show_rows(ui, ROW, count + usize::from(more), |ui, visible| {
            for i in visible {
                if i == count {
                    let size = vec2(ui.available_width(), ROW);
                    let layout = egui::Layout::top_down(egui::Align::Center);
                    ui.allocate_ui_with_layout(size, layout, |ui| {
                        load_more = icon_text_button(ui, Icon::Plus, "Cargar más", true).clicked();
                    });
                } else {
                    self.row(ui, i, &needle, now, hovered, actions);
                }
            }
        });
        if self.hovered_lane != hovered {
            ui.ctx().request_repaint();
        }
        load_more
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        i: usize,
        needle: &str,
        now: i64,
        hovered: Option<usize>,
        actions: &mut Actions,
    ) {
        let selected = self.selected == Some(i);
        let (rect, response) = selectable_row(ui, ROW, selected);
        if response.clicked() {
            self.selected = Some(i);
        }
        response.context_menu(|ui| actions.commit_menu(ui, &self.graph.commits[i].id));
        let lane = self.graph.rows[i].lane;
        if response.hovered() {
            self.hovered_lane = Some(lane);
        }
        let commit = &self.graph.commits[i];
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, selected, &commit.subject)
        });

        let opacity = if needle.is_empty() || matches(commit, needle) {
            1.0
        } else {
            0.3
        };
        let graph_x = rect.min.x + self.refs_w;
        let graph_w = self.graph.lanes as f32 * LANE;
        let lanes = Rect::from_min_size(pos2(graph_x, rect.min.y), vec2(graph_w, ROW));
        draw_lanes(ui.painter(), lanes, &self.graph.rows, i, hovered);

        let mut painter = ui.painter().clone();
        painter.multiply_opacity(opacity);
        let (y, right) = (rect.center().y, rect.max.x - 8.0);
        let column = |from: f32, to: f32| {
            Rect::from_min_max(pos2(from, rect.min.y), pos2(to.max(from), rect.max.y))
        };
        let center = pos2(graph_x + (lane as f32 + 0.5) * LANE, y);
        let color = lane_color(lane).gamma_multiply(emphasis(lane, hovered).1);

        // Chips: alineados a la derecha de su columna; si no caben, se recortan los últimos.
        let chips: Vec<_> = commit
            .refs
            .iter()
            .map(|name| ref_kind(name))
            .filter(|&(kind, _)| {
                (kind != RefKind::Remote || self.show_remotes)
                    && (kind != RefKind::Tag || self.show_tags)
            })
            .map(|(kind, text)| (kind, text, chip(&painter, kind, text)))
            .collect();
        if !chips.is_empty() {
            let (left, end) = (rect.min.x + 4.0, graph_x - 4.0);
            let total: f32 = chips
                .iter()
                .map(|(_, _, (width, _))| width + 4.0)
                .sum::<f32>()
                - 4.0;
            // La línea va antes que el nodo, que la tapa en la punta.
            painter.line_segment([pos2(end, y), center], Stroke::new(1.0, color));
            let clipped = painter.with_clip_rect(column(left, end).intersect(painter.clip_rect()));
            let mut x = (end - total).max(left);
            for (k, (kind, text, (width, mut shapes))) in chips.into_iter().enumerate() {
                for shape in &mut shapes {
                    shape.translate(vec2(x, y));
                }
                clipped.extend(shapes);
                // Cada chip responde al clic derecho (y el de una rama local, al doble clic).
                let area = Rect::from_min_size(pos2(x, y - 9.0), vec2(width, 18.0))
                    .intersect(column(left, end));
                let target = match kind {
                    RefKind::Head => text
                        .strip_prefix("HEAD -> ")
                        .map(|b| Target::Local(b.into())),
                    RefKind::Orion | RefKind::Local => Some(Target::Local(text.into())),
                    RefKind::Remote => Some(Target::Remote(text.into())),
                    RefKind::Tag => Some(Target::Tag(text.into())),
                };
                if let Some(target) = target.filter(|_| area.is_positive()) {
                    let chip =
                        ui.interact(area, ui.make_persistent_id(("chip", i, k)), Sense::click());
                    if chip.clicked() {
                        self.selected = Some(i);
                    }
                    if let Target::Local(branch) = &target
                        && chip.double_clicked()
                    {
                        actions.checkout(ui.ctx(), branch);
                    }
                    chip.context_menu(|ui| actions.ref_menu(ui, &target));
                }
                x += width + 4.0;
            }
        }
        node(&painter, center, commit, color);

        // Sin columna de autor: el asunto tiene más espacio. El nombre completo va en el
        // tooltip del nodo, que es donde están sus iniciales.
        let text_end = right - DATE_W - 8.0;
        let x = lanes.max.x + 8.0;
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
            column(right - DATE_W, right),
            ago(now, commit.time),
            small,
            theme().muted(),
        );

        let over_node = ui.input(|i| i.pointer.hover_pos()).is_some_and(|pos| {
            Rect::from_center_size(center, vec2(NODE * 2.0, NODE * 2.0)).contains(pos)
        });
        if response.hovered() && over_node {
            let _ = response.on_hover_text(&commit.author);
        }
    }
}

/// Cuántos commits se leen de entrada, y cuántos más con «Cargar más».
const LIMIT: usize = 2000;
/// Cada cuánto se mira si el repo cambió, mientras la pestaña está a la vista.
const FINGERPRINT_EVERY: Duration = Duration::from_secs(3);

/// Lo que trae una carga: el grafo, las refs y el WIP, más la huella del repo al empezar.
struct Loaded {
    print: String,
    graph: Graph,
    refs: Refs,
    wip: Vec<FileChange>,
    /// Estado, tracking y último asunto, para las acciones.
    state: graph_actions::State,
}

/// Una pestaña "Grafo": el grafo de un repo, leído con git en un hilo aparte.
pub(super) struct GraphTab {
    repo: PathBuf,
    /// None hasta que termina la primera carga.
    view: Option<GraphView>,
    /// Una carga en camino.
    loading: Option<Receiver<Result<Loaded, String>>>,
    /// La última carga falló (se sigue mostrando el grafo anterior, si había).
    error: Option<String>,
    /// Era la pestaña activa en el frame anterior (para recargar cuando vuelve a serlo).
    shown: bool,
    /// Los paneles de refs (izquierda) y detalle (derecha).
    panels: Panels,
    /// Las acciones de git (barra, panel de commit, menús y diálogos).
    actions: Actions,
    /// Cuántos commits se leen (crece con «Cargar más»).
    limit: usize,
    /// Solo los commits de esta rama; `None` = todas.
    branch: Option<String>,
    /// Huella del repo en la última carga, y una huella nueva en camino.
    print: Option<String>,
    printing: Option<Receiver<Result<String, String>>>,
    /// Cuándo se pidió la última huella.
    printed: Instant,
}

impl GraphTab {
    /// Una pestaña vacía: carga el grafo la primera vez que queda activa.
    pub(super) fn new(repo: PathBuf) -> Self {
        Self {
            panels: Panels::new(repo.clone()),
            actions: Actions::new(repo.clone()),
            repo,
            view: None,
            loading: None,
            error: None,
            shown: false,
            limit: LIMIT,
            branch: None,
            print: None,
            printing: None,
            printed: Instant::now(),
        }
    }

    /// Vuelve a leer grafo, refs y WIP en un hilo (si ya hay una carga en camino, no hace nada).
    fn reload(&mut self, ctx: &egui::Context) {
        if self.loading.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (repo, ctx) = (self.repo.clone(), ctx.clone());
        let (branch, limit) = (self.branch.clone(), self.limit);
        std::thread::spawn(move || {
            // La huella va primero: si el repo cambia durante la carga, la próxima lo nota.
            let print = git_graph::fingerprint(&repo).unwrap_or_default();
            let loaded = git_graph::load_rev(&repo, branch.as_deref(), limit).map(|graph| Loaded {
                print,
                graph,
                // Sin refs o sin WIP el grafo igual sirve.
                refs: git_graph::refs(&repo).unwrap_or_default(),
                wip: git_graph::wip(&repo).unwrap_or_default(),
                state: graph_actions::load(&repo),
            });
            let _ = tx.send(loaded);
            ctx.request_repaint(); // para que `poll` lo vea aunque nada más pase
        });
        self.loading = Some(rx);
    }

    /// Como `reload`, pero descarta la carga en camino (leía otra rama u otro límite).
    fn restart(&mut self, ctx: &egui::Context) {
        self.loading = None;
        self.reload(ctx);
    }

    /// Cada frame: recarga si la pestaña vuelve a estar a la vista, recoge lo que terminó y,
    /// mientras se ve, cada `FINGERPRINT_EVERY` mira si el repo cambió.
    pub(super) fn poll(&mut self, ctx: &egui::Context, shown: bool) {
        if shown && !self.shown {
            self.reload(ctx);
        }
        self.shown = shown;
        self.panels.poll();
        // Terminó una operación de git: se recarga ya, aunque hubiera una carga en camino.
        if self.actions.poll() {
            self.restart(ctx);
        }

        let result = self.loading.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("la carga se interrumpió".into())),
        });
        if let Some(result) = result {
            self.loading = None;
            match result {
                Ok(loaded) => {
                    self.error = None;
                    self.print = Some(loaded.print);
                    self.actions.set_state(loaded.state, &loaded.refs);
                    self.panels.refs = loaded.refs;
                    self.panels.set_wip(loaded.wip);
                    match &mut self.view {
                        Some(view) => view.replace(loaded.graph),
                        None => self.view = Some(GraphView::new(loaded.graph)),
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }

        let print = self.printing.as_ref().map(|rx| rx.try_recv());
        match print {
            None | Some(Err(TryRecvError::Empty)) => {}
            Some(Err(TryRecvError::Disconnected)) => self.printing = None,
            Some(Ok(print)) => {
                self.printing = None;
                // Si falló, se reintenta en la próxima vuelta.
                if let Ok(print) = print
                    && self.print.as_ref() != Some(&print)
                {
                    self.reload(ctx);
                }
            }
        }
        // Con una huella o una carga en camino no se pide otra: su hilo despierta la ventana.
        if shown && self.printing.is_none() && self.loading.is_none() {
            if self.printed.elapsed() >= FINGERPRINT_EVERY {
                let (tx, rx) = mpsc::channel();
                let (repo, ctx) = (self.repo.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(git_graph::fingerprint(&repo));
                    ctx.request_repaint();
                });
                self.printing = Some(rx);
                self.printed = Instant::now();
            }
            ctx.request_repaint_after(FINGERPRINT_EVERY.saturating_sub(self.printed.elapsed()));
        }
    }

    /// La carpeta donde se pidió abrir una terminal (menú de un worktree), si alguna.
    pub(super) fn take_terminal(&mut self) -> Option<PathBuf> {
        self.actions.terminal.take()
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
        let selected = view.selected.map(|i| view.graph.commits[i].id.clone());
        self.actions.bar(ui, selected.as_deref());
        self.actions.messages(ui);
        let refresh = view.toolbar(ui, busy);
        ui.separator();
        // Los paneles van antes que la lista: así la lista ocupa lo que sobra.
        let branch = self
            .panels
            .left(ui, view, self.branch.as_deref(), &mut self.actions);
        self.panels.right(ui, view, &mut self.actions);
        self.actions.conflict_band(ui);
        self.panels.header(ui, view);
        let more = view.list(
            ui,
            view.graph.commits.len() >= self.limit,
            &mut self.actions,
        );
        self.actions.dialogs(ui.ctx());

        let ctx = ui.ctx().clone();
        if let Some(branch) = branch {
            self.branch = branch;
            self.restart(&ctx);
        } else if more {
            self.limit += LIMIT;
            self.restart(&ctx);
        } else if refresh {
            self.reload(&ctx);
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Datos de ejemplo: `main` con una rama lateral que se fusiona.
    pub(in crate::app) fn sample() -> Graph {
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
    fn author_initials() {
        assert_eq!(initials("Ana Pérez"), "AP");
        assert_eq!(initials("juan de la cruz"), "JC");
        assert_eq!(initials("Claude"), "C");
        assert_eq!(initials("  (élise)  "), "É");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn author_colors_are_stable() {
        assert_eq!(author_color("Ana Pérez"), author_color("Ana Pérez"));
        // Valor fijo (FNV-1a("a") = 0xaf63dc4c8601ec8c): si cambia el hash, los autores
        // cambian de color entre versiones.
        let index = (0xaf63_dc4c_8601_ec8c_u64 % AUTHOR_COLORS.len() as u64) as usize;
        assert_eq!(author_color("a"), AUTHOR_COLORS[index]);
    }

    #[test]
    fn view_draws_without_input() {
        let mut view = GraphView::new(sample());
        view.selected = Some(1);
        view.filter = "merge".into();
        view.hovered_lane = Some(1);
        let (mut refresh, mut more) = (true, true);
        let mut output = egui::Context::default().run_ui(Default::default(), |ui| {
            refresh = view.toolbar(ui, false);
            more = view.list(ui, true, &mut Actions::new(PathBuf::from(".")));
        });
        assert!(!refresh && !more);
        // Los cambios de carril salen como curvas.
        assert!(
            output
                .shapes
                .iter()
                .any(|s| matches!(s.shape, Shape::CubicBezier(_)))
        );
        // Sin puntero no queda ningún carril resaltado.
        assert_eq!(view.hovered_lane, None);
        output.textures_delta.clear();
    }

    #[test]
    fn list_has_a_header_and_no_author_column() {
        let mut view = GraphView::new(sample());
        let mut output = egui::Context::default().run_ui(Default::default(), |ui| {
            view.list(ui, false, &mut Actions::new(PathBuf::from(".")));
        });
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for expected in ["RAMA / TAG", "GRAFO", "MENSAJE", "FECHA"] {
            assert!(texts.iter().any(|t| t == expected), "{expected}: {texts:?}");
        }
        // El autor completo ya no ocupa una columna: solo quedan sus iniciales en el nodo.
        assert!(!texts.iter().any(|t| t == "Ana Pérez" || t == "Claude"));
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
