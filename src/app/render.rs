//! Dibujar la terminal: celdas con sus colores y estilos, cursor, selección y mouse.

use crate::app::Orion;
use crate::colors::BG;
use crate::colors::FG;
use crate::colors::SELECTION;
use crate::colors::cell_colors;
use crate::colors::readable;
use crate::colors::resolve;
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
use std::time::Duration;

/// Medio ciclo de parpadeo (encendido o apagado), el mismo valor que usa Windows por defecto.
pub(crate) const BLINK: Duration = Duration::from_millis(530);

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

impl Orion {
    /// Grilla, cursor, selección y menú de clic derecho.
    pub(crate) fn draw_terminal(&mut self, ui: &mut egui::Ui) {
        egui::Frame::central_panel(ui.style())
            .fill(BG)
            .show(ui, |ui| {
                let size = self.settings.font.size;
                // Fuente monoespaciada: todas las letras miden lo mismo, así que 'M' sirve de celda.
                let font = egui::FontId::new(size, egui::FontFamily::Monospace);
                let bold_font = egui::FontId::new(size, bold_family());
                let cell = ui
                    .ctx()
                    .fonts_mut(|f| egui::vec2(f.glyph_width(&font, 'M'), f.row_height(&font)));
                let grid = grid_size(ui.available_size(), cell);
                if grid != self.grid {
                    self.grid = grid;
                    // La grilla y la shell tienen que coincidir: si no, la shell corta líneas donde no debe.
                    self.term.resize(term_size(grid));
                    let _ = self.shell.master.resize(pty_size(grid));
                }

                let (rect, response) =
                    ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

                // ¿La app pidió el mouse? (vim, btop, lazygit…) Shift+mouse sigue seleccionando, como en otras terminales.
                let mode = *self.term.mode();
                let shift = ui.input(|i| i.modifiers.shift);
                let mouse_mode = mode.intersects(TermMode::MOUSE_MODE) && !shift && !self.menu.open;
                let sgr = mode.contains(TermMode::SGR_MOUSE);
                let cell_at = |pos| {
                    let (point, _) = pixel_to_point(pos, rect.min, cell, grid, 0);
                    (point.column.0, point.line.0 as usize)
                };
                let mut report = Vec::new();

                // Rueda del mouse: positivo = hacia arriba (historial más viejo).
                // Con el menú abierto, la rueda es para su lista de fuentes.
                let delta = if self.menu.open {
                    0.0
                } else {
                    ui.input(|i| i.smooth_scroll_delta.y)
                };
                let lines = scroll_lines(&mut self.scroll_acc, delta, cell.y);
                let hover = ui.input(|i| i.pointer.hover_pos());
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
                            } if rect.contains(pos) || !pressed => {
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
                            egui::Event::PointerMoved(pos) if rect.contains(pos) => {
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

                // Clic derecho: menú con copiar, pegar y configuración (si la app no pidió el mouse).
                if !mouse_mode {
                    response.context_menu(|ui| {
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
                        if ui.button("Configuración…   Ctrl+,").clicked() {
                            self.open_menu();
                            ui.close();
                        }
                    });
                }

                // --- Selección con el mouse ---
                let offset = self.term.grid().display_offset();
                let to_point = |pos| pixel_to_point(pos, rect.min, cell, grid, offset);
                if let (false, Some(pos)) = (mouse_mode, response.interact_pointer_pos()) {
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

                let painter = ui.painter();
                let colors = self.term.colors();
                let selected = self.term.renderable_content().selection;
                for (row, cells) in visible_rows(&self.term).iter().enumerate() {
                    // Cada fila se dibuja en una posición exacta: fila * alto de celda.
                    let origin = rect.min + egui::vec2(0.0, row as f32 * cell.y);
                    let mut text = LayoutJob::default();

                    for (col, c) in cells.iter().enumerate() {
                        let (mut fg, mut bg) = cell_colors(c, colors);
                        let point = Point::new(Line(row as i32 - offset as i32), Column(col));
                        if selected.is_some_and(|s| s.contains(point)) {
                            bg = SELECTION;
                            fg = readable(fg, bg);
                        }
                        if bg != BG {
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

                    painter.galley(origin, painter.layout_job(text), FG);
                }
                // El cursor usa el color "Cursor" del Term (las apps pueden cambiarlo con OSC 12).
                let color = resolve(Color::Named(NamedColor::Cursor), colors);

                // --- Cursor ---
                let content = self.term.renderable_content();
                let (cursor, offset) = (content.cursor, content.display_offset);
                let focused = ui.input(|i| i.focused);
                let blinking = self.term.cursor_style().blinking && focused;
                let since = self.blink_epoch.elapsed();
                if blinking {
                    // egui duerme si nada cambia: le pedimos despertar justo en el próximo cambio de fase.
                    let phase =
                        Duration::from_millis((since.as_millis() % BLINK.as_millis()) as u64);
                    ui.ctx().request_repaint_after(BLINK - phase);
                }
                // Sin foco, cursor hueco y fijo (como Alacritty/Ghostty): se ve dónde estás, sin distraer.
                let shape = match cursor.shape {
                    CursorShape::Hidden => CursorShape::Hidden,
                    _ if !focused => CursorShape::HollowBlock,
                    shape => shape,
                };
                let row = cursor.point.line.0 + offset as i32;
                // Si scrolleaste hacia arriba (#12), el cursor puede quedar fuera de la vista.
                let in_view = (0..self.grid.1 as i32).contains(&row);
                if shape != CursorShape::Hidden && in_view && cursor_on(blinking, since) {
                    let min = rect.min
                        + egui::vec2(cursor.point.column.0 as f32 * cell.x, row as f32 * cell.y);
                    let r = egui::Rect::from_min_size(min, cell);
                    let thin = 2.0;
                    match shape {
                        CursorShape::Block => {
                            // Bloque relleno y, encima, la letra que tapa con el color del fondo.
                            painter.rect_filled(r, 0.0, color);
                            let c = self.term.grid()[cursor.point].c;
                            painter.text(r.min, egui::Align2::LEFT_TOP, c, font.clone(), BG);
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
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
