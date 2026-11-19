//! Dibujar la terminal: celdas con sus colores y estilos, cursor, selección y mouse.

use super::session::Session;
use crate::blocks::{
    Status, format_duration, logical_line, visible_logical_lines, visible_rows_of,
};
use crate::colors::cell_colors;
use crate::colors::readable;
use crate::colors::resolve;
use crate::colors::surface;
use crate::colors::theme;
use crate::config::FontSettings;
use crate::fonts::bold_family;
use crate::grid::grid_size;
use crate::grid::is_spacer;
use crate::grid::pixel_to_point;
use crate::grid::term_size;
use crate::grid::visible_rows;
use crate::input::mouse_report;
use crate::input::scroll_lines;
use crate::pty::pty_size;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::Column;
use alacritty_terminal::index::Line;
use alacritty_terminal::index::Point;
use alacritty_terminal::selection::Selection;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::Color;
use alacritty_terminal::vte::ansi::CursorShape;
use alacritty_terminal::vte::ansi::NamedColor;
use eframe::egui;
use eframe::egui::text::ByteIndex;
use eframe::egui::text::LayoutJob;
use eframe::egui::text::LayoutSection;
use eframe::egui::text::TextFormat;
use std::io::Write;
use std::time::{Duration, Instant};

/// Medio ciclo de parpadeo (encendido o apagado), el mismo valor que usa Windows por defecto.
pub(crate) const BLINK: Duration = Duration::from_millis(530);

/// Margen a la izquierda del texto: ahí va la barra de color de cada bloque.
const GUTTER: f32 = 16.0;
/// Aire entre el texto y los bordes de la ventana (arriba, derecha y abajo).
const PADDING: f32 = 10.0;

/// Borde de arriba de cada fila en pantalla. Antes de cada fila de `gap_rows` (prompts de bloques)
/// se deja `gap` de aire, salvo en la fila 0. Las filas con `hidden[fila]` (salida de un bloque
/// colapsado) no ocupan lugar. Si con todo eso la fila `keep` (la del cursor) quedaría debajo
/// de `bottom`, todo sube lo necesario: igual que cuando la salida hace scroll.
fn row_tops(
    cell_h: f32,
    gap: f32,
    gap_rows: &[usize],
    hidden: &[bool],
    keep: Option<usize>,
    (top, bottom): (f32, f32),
) -> Vec<f32> {
    let rows = hidden.len();
    let mut y = top;
    let mut tops = Vec::with_capacity(rows);
    for (row, &is_hidden) in hidden.iter().enumerate() {
        if row > 0 && gap_rows.contains(&row) {
            y += gap;
        }
        tops.push(y);
        if !is_hidden {
            y += cell_h;
        }
    }
    if let Some(row) = keep.filter(|&r| r < rows) {
        let overflow = tops[row] + cell_h - bottom;
        if overflow > 0.0 {
            tops.iter_mut().for_each(|t| *t -= overflow);
        }
    }
    tops
}

/// Qué fila hay en la altura `y`. El aire entre bloques cuenta como parte de la fila de arriba.
fn row_at(tops: &[f32], y: f32) -> usize {
    tops.partition_point(|&t| t <= y).saturating_sub(1)
}

/// Color de un bloque según su estado: verde bien, rojo error, amarillo corriendo, gris escribiendo.
pub(super) fn status_color(status: Status) -> egui::Color32 {
    match status {
        Status::Editing => theme().ansi[8],
        Status::Running(_) => theme().ansi[3],
        Status::Done(Some(0), _) | Status::Done(None, _) => theme().ansi[2],
        Status::Done(Some(_), _) => theme().ansi[1],
    }
}

/// Texto de la etiqueta de la derecha: "✓ 120 ms", "✗ 3 · 1.2 s", "● 4.0 s" mientras corre.
fn status_label(status: Status) -> Option<String> {
    Some(match status {
        Status::Editing => return None,
        Status::Running(t) => format!("● {}", format_duration(t)),
        Status::Done(Some(0) | None, t) => format!("✓ {}", format_duration(t)),
        Status::Done(Some(code), t) => format!("✗ {code} · {}", format_duration(t)),
    })
}

/// Agrega un carácter al LayoutJob. Si tiene el mismo estilo que el anterior, extiende esa sección:
/// así egui maneja unos pocos tramos por fila en vez de una sección por letra.
pub(crate) fn push_char(job: &mut LayoutJob, c: char, format: TextFormat) {
    let start = ByteIndex(job.text.len());
    job.text.push(c);
    let end = ByteIndex(job.text.len());
    match job.sections.last_mut() {
        Some(s) if s.format == format => s.byte_range.end = end,
        _ => job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: start..end,
            format,
        }),
    }
}

/// ¿El cursor está en su fase "encendida"? Alterna cada BLINK desde `since` (el último input).
pub(crate) fn cursor_on(blinking: bool, since: Duration) -> bool {
    !blinking || (since.as_millis() / BLINK.as_millis()).is_multiple_of(2)
}

/// Un bloque tal como se ve ahora en pantalla.
#[derive(Clone, Copy)]
struct BlockView {
    /// Posición en `Blocks::list`.
    index: usize,
    status: Status,
    /// Primera y última fila visible del bloque (inclusive).
    first: usize,
    last: usize,
    /// ¿Se ve el prompt? (si no, el bloque empezó más arriba de la pantalla).
    prompt_visible: bool,
    /// Primera fila visible de la salida, si se ve.
    output_row: Option<usize>,
    collapsed: bool,
    output_lines: usize,
}

/// Algo que se pidió sobre un bloque (con el mouse o el teclado).
pub(super) enum BlockAction {
    CopyCommand(usize),
    CopyOutput(usize),
    ToggleCollapse(usize),
}

/// El aviso breve ("Salida copiada"), abajo al centro de `area`, durante 1.5 s.
pub(super) fn draw_toast(ui: &egui::Ui, area: egui::Rect, toast: &(String, Instant), size: f32) {
    let (text, since) = toast;
    if since.elapsed().as_secs_f32() > 1.5 {
        return;
    }
    let small = egui::FontId::new(size * 0.8, egui::FontFamily::Monospace);
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(text.clone(), small, theme().fg);
    let pos = egui::pos2(
        area.center().x - galley.size().x / 2.0,
        area.max.y - galley.size().y - 16.0,
    );
    let r = egui::Rect::from_min_size(pos, galley.size()).expand2(egui::vec2(10.0, 4.0));
    painter.rect_filled(r, 6.0, theme().selection);
    painter.galley(pos, galley, theme().fg);
    ui.ctx().request_repaint_after(Duration::from_millis(100));
}

impl Session {
    /// Bloques que se ven ahora, de arriba hacia abajo.
    /// En apps de pantalla completa (vim, less) no hay bloques: la pantalla alternativa no tiene prompts.
    fn block_views(&self) -> Vec<BlockView> {
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return Vec::new();
        }
        let lines = visible_logical_lines(&self.term);
        let cursor_line = logical_line(&self.term, self.term.grid().cursor.point.line);
        let views = self
            .blocks
            .list
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                let span = block.span(cursor_line);
                let (first, last) = visible_rows_of(&lines, span)?;
                let output_row = block
                    .output
                    .and_then(|out| visible_rows_of(&lines, (out, span.1)))
                    .map(|(row, _)| row);
                Some(BlockView {
                    index,
                    status: block.status(),
                    first,
                    last,
                    prompt_visible: lines[first] == block.prompt,
                    output_row,
                    collapsed: block.collapsed,
                    output_lines: block.output_lines(),
                })
            });
        views.collect()
    }

    /// Aplica lo que se pidió sobre un bloque: copiar (con aviso) o colapsar/expandir.
    pub(super) fn apply_block_action(&mut self, ctx: &egui::Context, action: BlockAction) {
        let (text, what) = match action {
            BlockAction::ToggleCollapse(i) => {
                if let Some(block) = self.blocks.list.get_mut(i) {
                    block.collapsed = !block.collapsed;
                }
                return;
            }
            BlockAction::CopyCommand(i) => {
                let text = self.blocks.list.get(i).map(|b| b.command.clone());
                (text, "Comando copiado")
            }
            BlockAction::CopyOutput(i) => {
                let text = self
                    .blocks
                    .list
                    .get(i)
                    .and_then(|b| b.output_text(&self.term));
                (text, "Salida copiada")
            }
        };
        self.toast = Some(match text {
            Some(text) => {
                ctx.copy_text(text);
                what.into()
            }
            None => "Esa salida ya no está en el historial".into(),
        });
    }

    /// Grilla, cursor, selección y menú de clic derecho.
    /// `menu_open`: la ventana de configuración está abierta (el teclado y la rueda son para ella).
    /// `active`: es el panel con el foco (los otros splits muestran el cursor hueco).
    /// Devuelve true si se pidió abrir la configuración desde el menú de clic derecho.
    pub(super) fn draw(
        &mut self,
        ui: &mut egui::Ui,
        settings: &FontSettings,
        menu_open: bool,
        active: bool,
    ) -> bool {
        let mut open_menu = false;
        egui::Frame::central_panel(ui.style())
            .fill(surface(theme().bg)) // semitransparente con Mica o acrílico
            .show(ui, |ui| {
                let size = settings.size;
                // Fuente monoespaciada: todas las letras miden lo mismo, así que 'M' sirve de celda.
                let font = egui::FontId::new(size, egui::FontFamily::Monospace);
                let bold_font = egui::FontId::new(size, bold_family());
                let (width, text_height) = ui
                    .ctx()
                    .fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
                // Interlineado: la fila es más alta que la letra y la letra va centrada en ella.
                // Redondeado a píxeles enteros para que las fuentes pixeladas no se vean borrosas.
                let cell = egui::vec2(width, (text_height * settings.line_height).round());
                let text_dy = ((cell.y - text_height) / 2.0).round();
                let margins = egui::vec2(GUTTER + PADDING, 2.0 * PADDING);
                let grid = grid_size(ui.available_size() - margins, cell);
                if grid != self.grid {
                    self.grid = grid;
                    // La grilla y la shell tienen que coincidir: si no, la shell corta líneas donde no debe.
                    self.term.resize(term_size(grid));
                    let _ = self.shell.master.resize(pty_size(grid));
                }

                // `area` = toda la superficie (para el mouse); `rect` = donde va el texto, después del margen.
                let (area, response) =
                    ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
                let rect = egui::Rect::from_min_max(
                    area.min + egui::vec2(GUTTER, PADDING),
                    area.max - egui::vec2(PADDING, PADDING),
                );

                // El teclado es de la terminal, como en un cuadro de texto. Sin esto, egui usa Tab para
                // mover el foco entre botones: al autocompletar con Tab, el foco caía en "+ Nueva pestaña"
                // y el Enter siguiente lo "apretaba". Con el menú o la paleta abiertos, el foco es de ellos.
                if active && !menu_open {
                    response.request_focus();
                    let keys = egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    };
                    ui.memory_mut(|m| m.set_focus_lock_filter(response.id, keys));
                }

                // ¿La app pidió el mouse? (vim, btop, lazygit…) Shift+mouse sigue seleccionando, como en otras terminales.
                let mode = *self.term.mode();
                let shift = ui.input(|i| i.modifiers.shift);
                let mouse_mode = mode.intersects(TermMode::MOUSE_MODE) && !shift && !menu_open;
                let sgr = mode.contains(TermMode::SGR_MOUSE);
                let cell_at = |pos| {
                    let (point, _) = pixel_to_point(pos, rect.min, cell, grid, 0);
                    (point.column.0, point.line.0 as usize)
                };
                let mut report = Vec::new();

                // Rueda del mouse: positivo = hacia arriba (historial más viejo).
                // Con el menú abierto, la rueda es para su lista de fuentes.
                // Con splits, la rueda es solo para el panel que está bajo el mouse.
                let hover = ui.input(|i| i.pointer.hover_pos());
                let delta = if menu_open || !hover.is_some_and(|p| area.contains(p)) {
                    0.0
                } else {
                    ui.input(|i| i.smooth_scroll_delta.y)
                };
                let lines = scroll_lines(&mut self.scroll_acc, delta, cell.y);
                if lines != 0 {
                    let alt_scroll = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
                    if let (true, Some(pos)) = (mouse_mode, hover) {
                        // La app recibe la rueda como botones 64 (arriba) y 65 (abajo).
                        let (col, row) = cell_at(pos);
                        let button = if lines > 0 { 64 } else { 65 };
                        for _ in 0..lines.unsigned_abs() {
                            report.extend(mouse_report(button, col, row, true, sgr));
                        }
                    } else if mode.contains(alt_scroll) {
                        // Apps de pantalla completa sin mouse (less, man): la rueda se vuelve flechas.
                        let arrow: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
                        report.extend(arrow.repeat(lines.unsigned_abs() as usize));
                    } else {
                        self.term.scroll_display(Scroll::Delta(lines));
                    }
                }

                if mouse_mode {
                    let mods = ui.input(|i| i.modifiers);
                    let extra = 8 * mods.alt as u8 + 16 * mods.ctrl as u8;
                    for event in ui.input(|i| i.events.clone()) {
                        match event {
                            // Apretar adentro de la terminal; soltar se informa aunque sea afuera.
                            egui::Event::PointerButton {
                                pos,
                                button,
                                pressed,
                                ..
                            } if area.contains(pos)
                                || (!pressed && self.mouse_button.is_some()) =>
                            {
                                let code = match button {
                                    egui::PointerButton::Primary => 0,
                                    egui::PointerButton::Middle => 1,
                                    egui::PointerButton::Secondary => 2,
                                    _ => continue,
                                };
                                self.mouse_button = pressed.then_some(code);
                                let (col, row) = cell_at(pos);
                                report.extend(mouse_report(code + extra, col, row, pressed, sgr));
                            }
                            egui::Event::PointerMoved(pos) if area.contains(pos) => {
                                let at = cell_at(pos);
                                // Solo al cambiar de celda: moverse dentro de la misma no le dice nada nuevo a la app.
                                if self.mouse_cell == Some(at) {
                                    continue;
                                }
                                self.mouse_cell = Some(at);
                                let drag = TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION;
                                let code = match self.mouse_button {
                                    Some(b) if mode.intersects(drag) => b + 32, // arrastrando
                                    None if mode.contains(TermMode::MOUSE_MOTION) => 3 + 32, // moverse sin botón
                                    _ => continue,
                                };
                                report.extend(mouse_report(code + extra, at.0, at.1, true, sgr));
                            }
                            _ => {}
                        }
                    }
                }
                if !report.is_empty() {
                    let _ = self.shell.writer.borrow_mut().write_all(&report);
                }

                // --- Dónde va cada fila: con aire antes de cada bloque ---
                let views = self.block_views();
                let gap = (cell.y * 0.6).round();
                let gap_rows: Vec<usize> = views
                    .iter()
                    .filter(|v| v.prompt_visible)
                    .map(|v| v.first)
                    .collect();
                let rows = grid.1 as usize;
                // La salida de los bloques colapsados no ocupa lugar.
                let mut hidden = vec![false; rows];
                for view in views.iter().filter(|v| v.collapsed) {
                    if let Some(out) = view.output_row {
                        hidden[out..=view.last].fill(true);
                    }
                }
                let offset = self.term.grid().display_offset();
                let cursor_row = self.term.grid().cursor.point.line.0 + offset as i32;
                let keep = usize::try_from(cursor_row).ok();
                let tops = row_tops(
                    cell.y,
                    gap,
                    &gap_rows,
                    &hidden,
                    keep,
                    (rect.min.y, rect.max.y),
                );
                let row_bottom = |row: usize| tops[row] + if hidden[row] { 0.0 } else { cell.y };

                // Bloque bajo el mouse: para sus botones y para el menú de clic derecho.
                let hovered = hover
                    .filter(|pos| area.contains(*pos) && !mouse_mode)
                    .and_then(|pos| {
                        let row = row_at(&tops, pos.y);
                        views
                            .iter()
                            .find(|v| (v.first..=v.last).contains(&row))
                            .copied()
                    });
                if response.secondary_clicked() {
                    self.context_block = hovered.map(|v| v.index);
                }
                let mut actions = Vec::new();
                let marked = self.marked_block;

                // Clic derecho: acciones del bloque (si hay uno ahí), copiar, pegar y configuración.
                if !mouse_mode {
                    response.context_menu(|ui| {
                        let block = self
                            .context_block
                            .and_then(|i| Some((i, self.blocks.list.get(i)?)));
                        if let Some((i, block)) =
                            block.filter(|(_, b)| matches!(b.status(), Status::Done(..)))
                        {
                            if ui.button("Copiar comando").clicked() {
                                actions.push(BlockAction::CopyCommand(i));
                                ui.close();
                            }
                            if ui.button("Copiar salida").clicked() {
                                actions.push(BlockAction::CopyOutput(i));
                                ui.close();
                            }
                            let toggle = if block.collapsed {
                                "Expandir bloque"
                            } else {
                                "Colapsar bloque"
                            };
                            if ui.button(toggle).clicked() {
                                actions.push(BlockAction::ToggleCollapse(i));
                                ui.close();
                            }
                            ui.separator();
                        }
                        let has_selection = self.term.selection.is_some();
                        if ui
                            .add_enabled(has_selection, egui::Button::new("Copiar"))
                            .clicked()
                        {
                            self.copy_selection(ui.ctx());
                            ui.close();
                        }
                        if ui.button("Pegar").clicked() {
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Dividir a la derecha   Ctrl+Shift+D").clicked() {
                            self.split_request = Some(true);
                            ui.close();
                        }
                        if ui.button("Dividir hacia abajo   Ctrl+Shift+E").clicked() {
                            self.split_request = Some(false);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Configuración…   Ctrl+,").clicked() {
                            open_menu = true;
                            ui.close();
                        }
                    });
                }

                // --- Selección con el mouse ---
                // El mouse está en coordenadas "con aire"; se pasan a una grilla pareja antes de buscar la celda.
                let to_point = |pos: egui::Pos2| {
                    let row = row_at(&tops, pos.y);
                    let dy = (pos.y - tops[row]).clamp(0.0, cell.y - 0.01);
                    let even = egui::pos2(pos.x, rect.min.y + row as f32 * cell.y + dy);
                    pixel_to_point(even, rect.min, cell, grid, offset)
                };
                // Ctrl + mouse sobre un link o una ruta: se subraya y el clic la abre.
                let ctrl = ui.input(|i| i.modifiers.ctrl);
                let link = hover
                    .filter(|p| ctrl && !mouse_mode && rect.contains(*p))
                    .and_then(|pos| {
                        let row = row_at(&tops, pos.y);
                        let col = ((pos.x - rect.min.x) / cell.x) as usize;
                        let screen = visible_rows(&self.term);
                        let cells = screen.get(row).filter(|c| col < c.len())?;
                        let (cols, link) = crate::links::link_at(cells, col, self.cwd.as_deref())?;
                        Some((row, cols, link))
                    });
                if link.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let underline = link.as_ref().map(|(row, cols, _)| (*row, cols.clone()));
                if let (true, Some((_, _, link))) = (response.clicked(), link) {
                    crate::links::open(link);
                } else if let (false, Some(pos)) = (mouse_mode, response.interact_pointer_pos()) {
                    let (point, side) = to_point(pos);
                    let start = |ty| Some(Selection::new(ty, point, side));
                    if response.triple_clicked() {
                        self.term.selection = start(SelectionType::Lines); // línea completa
                    } else if response.double_clicked() {
                        self.term.selection = start(SelectionType::Semantic); // palabra
                    } else if response.drag_started() {
                        // egui detecta el arrastre tras unos píxeles: la selección empieza donde apretaste.
                        let pressed = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
                        let (origin, origin_side) = to_point(pressed);
                        let mut selection =
                            Selection::new(SelectionType::Simple, origin, origin_side);
                        selection.update(point, side);
                        self.term.selection = Some(selection);
                    } else if response.dragged() {
                        if let Some(selection) = &mut self.term.selection {
                            selection.update(point, side);
                        }
                    } else if response.clicked() {
                        self.term.selection = None; // clic simple: deseleccionar
                    }
                }

                // Recortado a la zona del texto: si todo subió por el aire, lo de arriba no pisa el margen.
                let clip = egui::Rect::from_x_y_ranges(area.x_range(), rect.y_range());
                let painter = ui.painter().with_clip_rect(clip);
                let colors = self.term.colors();
                let selected = self.term.renderable_content().selection;

                // --- Bloques: debajo del texto van el tinte, el separador y la barra de color ---
                let rows_rect = |first: usize, last: usize| {
                    egui::Rect::from_x_y_ranges(area.x_range(), tops[first]..=row_bottom(last))
                };
                for view in &views {
                    let BlockView {
                        status,
                        first,
                        last,
                        prompt_visible,
                        ..
                    } = *view;
                    let r = rows_rect(first, last);
                    if marked == Some(view.index) {
                        // Bloque marcado con Ctrl+↑/↓: fondo un poco más claro y borde azul.
                        let glow = egui::Color32::from_rgba_unmultiplied(0x7a, 0xa2, 0xf7, 16);
                        painter.rect_filled(r, 4.0, glow);
                        let stroke = egui::Stroke::new(1.0, theme().ansi[4]);
                        painter.rect_stroke(r.shrink(1.0), 4.0, stroke, egui::StrokeKind::Inside);
                    } else if hovered.is_some_and(|h| h.index == view.index) {
                        // El bloque bajo el mouse se ilumina apenas: así se ve a cuál apuntan los botones.
                        let glow = egui::Color32::from_rgba_unmultiplied(0xc0, 0xca, 0xf5, 6);
                        painter.rect_filled(r, 0.0, glow);
                    }
                    if matches!(status, Status::Done(Some(code), _) if code != 0) {
                        // Un tinte rojo muy suave: los comandos que fallaron se encuentran de un vistazo.
                        let tint = egui::Color32::from_rgba_unmultiplied(0xf7, 0x76, 0x8e, 14);
                        painter.rect_filled(r, 0.0, tint);
                    }
                    if prompt_visible && first > 0 {
                        // En medio del aire: lejos de la salida de arriba y del prompt de abajo.
                        let y = (r.min.y - gap / 2.0).round();
                        painter.hline(area.x_range(), y, egui::Stroke::new(1.0, theme().line()));
                    }
                    let bar = egui::Rect::from_x_y_ranges(
                        area.min.x + 6.0..=area.min.x + 9.0,
                        r.min.y + 3.0..=r.max.y - 3.0,
                    );
                    painter.rect_filled(bar, 1.5, status_color(status));
                }

                let screen = visible_rows(&self.term);
                for (row, cells) in screen.iter().enumerate() {
                    if hidden[row] {
                        continue; // salida de un bloque colapsado
                    }
                    // Cada fila se dibuja en su posición: fila * alto de celda + el aire de los bloques de arriba.
                    let origin = egui::pos2(rect.min.x, tops[row]);
                    let mut text = LayoutJob::default();

                    for (col, c) in cells.iter().enumerate() {
                        let (mut fg, mut bg) = cell_colors(c, colors);
                        let point = Point::new(Line(row as i32 - offset as i32), Column(col));
                        if selected.is_some_and(|s| s.contains(point)) {
                            bg = theme().selection;
                            fg = readable(fg, bg);
                        }
                        if bg != theme().bg {
                            let width = if c.flags.contains(Flags::WIDE_CHAR) {
                                2.0
                            } else {
                                1.0
                            };
                            let min = origin + egui::vec2(col as f32 * cell.x, 0.0);
                            let size = egui::vec2(width * cell.x, cell.y);
                            painter.rect_filled(egui::Rect::from_min_size(min, size), 0.0, bg);
                        }
                        if is_spacer(c) {
                            continue;
                        }
                        let line = |on: bool| {
                            if on {
                                egui::Stroke::new(1.0, fg)
                            } else {
                                egui::Stroke::NONE
                            }
                        };
                        let format = TextFormat {
                            font_id: if c.flags.contains(Flags::BOLD) {
                                bold_font.clone()
                            } else {
                                font.clone()
                            },
                            color: fg,
                            italics: c.flags.contains(Flags::ITALIC),
                            // ponytail: los 5 tipos de subrayado (doble, ondulado…) se ven como uno simple.
                            underline: line(c.flags.intersects(Flags::ALL_UNDERLINES)),
                            strikethrough: line(c.flags.contains(Flags::STRIKEOUT)),
                            ..Default::default()
                        };
                        push_char(&mut text, c.c, format);
                    }

                    let text_pos = origin + egui::vec2(0.0, text_dy);
                    painter.galley(text_pos, painter.layout_job(text), theme().fg);
                }
                if let Some((row, cols)) = underline {
                    let x = |c: usize| rect.min.x + c as f32 * cell.x;
                    let y = tops[row] + text_dy + text_height;
                    painter.hline(
                        x(cols.start)..=x(cols.end),
                        y,
                        egui::Stroke::new(1.0, theme().ansi[4]),
                    );
                }

                // --- Bloques: encima del texto, la etiqueta con el resultado y lo que tardó ---
                let small = egui::FontId::new(size * 0.8, egui::FontFamily::Monospace);
                let mut running = false;
                for view in views.iter().filter(|v| v.prompt_visible) {
                    let Some(mut label) = status_label(view.status) else {
                        continue;
                    };
                    running |= matches!(view.status, Status::Running(_));
                    let done = matches!(view.status, Status::Done(..));
                    if view.collapsed {
                        label = format!("▸ {} líneas · {label}", view.output_lines);
                    }
                    let color = status_color(view.status);
                    let galley = painter.layout_no_wrap(label, small.clone(), color);
                    let row_top = tops[view.first];
                    let mut pos = egui::pos2(
                        rect.max.x - 4.0 - galley.size().x,
                        row_top + (cell.y - galley.size().y) / 2.0,
                    );
                    // Si el comando llega hasta donde va la etiqueta, la etiqueta sube al aire entre
                    // bloques (sobre la línea separadora) para no tapar el texto.
                    let text_end = screen[view.first]
                        .iter()
                        .rposition(|c| c.c != ' ' && !is_spacer(c))
                        .map_or(rect.min.x, |col| rect.min.x + (col + 1) as f32 * cell.x);
                    if view.first > 0 && text_end + 12.0 > pos.x {
                        pos.y = (row_top - gap / 2.0 - galley.size().y / 2.0).round();
                    }
                    let pill =
                        egui::Rect::from_min_size(pos, galley.size()).expand2(egui::vec2(6.0, 1.0));
                    // Clic en la etiqueta: colapsar o expandir el bloque.
                    let id = ui.scope_id().with(("bloque", view.index));
                    let click = ui.interact(pill, id, egui::Sense::click());
                    let border = if click.hovered() && done {
                        color
                    } else {
                        theme().line()
                    };
                    painter.rect_filled(pill, 4.0, theme().bg);
                    painter.rect_stroke(
                        pill,
                        4.0,
                        egui::Stroke::new(1.0, border),
                        egui::StrokeKind::Inside,
                    );
                    painter.galley(pos, galley, color);
                    if done {
                        let tip = if view.collapsed {
                            "Clic para expandir"
                        } else {
                            "Clic para colapsar"
                        };
                        if click
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            actions.push(BlockAction::ToggleCollapse(view.index));
                        }
                    }

                    // Bloque terminado bajo el mouse o marcado: botones para copiar, a la izquierda de la etiqueta.
                    let active = hovered.is_some_and(|h| h.index == view.index)
                        || marked == Some(view.index);
                    if done && active {
                        let mut right = pill.min.x - 6.0;
                        let buttons = [
                            (
                                "⧉ salida",
                                "Copiar la salida",
                                BlockAction::CopyOutput(view.index),
                            ),
                            (
                                "⧉ comando",
                                "Copiar el comando",
                                BlockAction::CopyCommand(view.index),
                            ),
                        ];
                        for (text, tip, action) in buttons {
                            let text = egui::RichText::new(text)
                                .font(small.clone())
                                .color(theme().ansi[7]);
                            let width = painter
                                .layout_no_wrap(text.text().into(), small.clone(), theme().fg)
                                .size()
                                .x;
                            let r = egui::Rect::from_min_max(
                                egui::pos2(right - width - 12.0, pill.min.y),
                                egui::pos2(right, pill.max.y),
                            );
                            let button = egui::Button::new(text)
                                .fill(theme().bg)
                                .stroke(egui::Stroke::new(1.0, theme().line()))
                                .corner_radius(4.0);
                            if ui.put(r, button).on_hover_text(tip).clicked() {
                                actions.push(action);
                            }
                            right = r.min.x - 4.0;
                        }
                    }
                }
                if running {
                    // El reloj del comando que corre se actualiza solo (egui no repinta si nada cambia).
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                }

                // El cursor usa el color "Cursor" del Term (las apps pueden cambiarlo con OSC 12).
                let color = resolve(Color::Named(NamedColor::Cursor), colors);

                // --- Cursor ---
                let content = self.term.renderable_content();
                let (cursor, offset) = (content.cursor, content.display_offset);
                // Con splits, solo el panel activo tiene el cursor "vivo".
                let focused = active && ui.input(|i| i.focused);
                let blinking = self.term.cursor_style().blinking && focused;
                let since = self.blink_epoch.elapsed();
                if blinking {
                    // egui duerme si nada cambia: le pedimos despertar justo en el próximo cambio de fase.
                    let phase =
                        Duration::from_millis((since.as_millis() % BLINK.as_millis()) as u64);
                    ui.ctx().request_repaint_after(BLINK - phase);
                }
                // Sin foco, cursor hueco y fijo: se ve dónde estás, sin distraer.
                let shape = match cursor.shape {
                    CursorShape::Hidden => CursorShape::Hidden,
                    _ if !focused => CursorShape::HollowBlock,
                    shape => shape,
                };
                let row = cursor.point.line.0 + offset as i32;
                // Si scrolleaste hacia arriba (#12), el cursor puede quedar fuera de la vista.
                let in_view = (0..self.grid.1 as i32).contains(&row);
                if shape != CursorShape::Hidden && in_view && cursor_on(blinking, since) {
                    let x = rect.min.x + cursor.point.column.0 as f32 * cell.x;
                    let min = egui::pos2(x, tops[row as usize]);
                    let r = egui::Rect::from_min_size(min, cell);
                    let thin = 2.0;
                    match shape {
                        CursorShape::Block => {
                            // Bloque relleno y, encima, la letra que tapa con el color del fondo.
                            painter.rect_filled(r, 0.0, color);
                            let c = self.term.grid()[cursor.point].c;
                            let pos = r.min + egui::vec2(0.0, text_dy);
                            painter.text(pos, egui::Align2::LEFT_TOP, c, font.clone(), theme().bg);
                        }
                        CursorShape::Beam => {
                            painter.rect_filled(r.with_max_x(r.min.x + thin), 0.0, color);
                        }
                        CursorShape::Underline => {
                            painter.rect_filled(r.with_min_y(r.max.y - thin), 0.0, color);
                        }
                        _ => {
                            let stroke = egui::Stroke::new(1.0, color);
                            painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
                        }
                    }
                }

                // Lo que se pidió sobre los bloques en este frame (botones, etiqueta o menú).
                for action in actions {
                    self.apply_block_action(ui.ctx(), action);
                }
            });
        open_menu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_leave_room_before_blocks() {
        // 4 filas de 10 px; un bloque empieza en la fila 2: ahí se suman 5 px de aire.
        let all = [false; 4];
        let tops = row_tops(10.0, 5.0, &[0, 2], &all, None, (0.0, 100.0));
        assert_eq!(tops, [0.0, 10.0, 25.0, 35.0]); // sin aire en la fila 0
        // El aire entre la fila 1 y la 2 cuenta como fila 1; arriba de todo, fila 0.
        assert_eq!(row_at(&tops, 21.0), 1);
        assert_eq!(row_at(&tops, 25.0), 2);
        assert_eq!(row_at(&tops, 9.9), 0);
        assert_eq!(row_at(&tops, -3.0), 0);
        // Si el cursor (fila 3) no cabe en 40 px, todo sube 5 px.
        let tops = row_tops(10.0, 5.0, &[2], &all, Some(3), (0.0, 40.0));
        assert_eq!(tops, [-5.0, 5.0, 20.0, 30.0]);
        // Filas 1 y 2 ocultas (bloque colapsado): no ocupan lugar, y el mouse cae en la fila de abajo.
        let tops = row_tops(
            10.0,
            0.0,
            &[],
            &[false, true, true, false],
            None,
            (0.0, 100.0),
        );
        assert_eq!(tops, [0.0, 10.0, 10.0, 10.0]);
        assert_eq!(row_at(&tops, 12.0), 3);
    }

    #[test]
    fn cursor_blinks_in_phases() {
        let ms = Duration::from_millis;
        assert!(cursor_on(true, ms(0))); // recién tecleado: visible
        assert!(cursor_on(true, ms(529)));
        assert!(!cursor_on(true, ms(530))); // segunda fase: apagado
        assert!(cursor_on(true, ms(1060)));
        assert!(cursor_on(false, ms(530))); // sin parpadeo: siempre visible
    }
}
