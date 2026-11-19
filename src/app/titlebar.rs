//! La barra de arriba: Orion dibuja la suya en vez de la de Windows. Logo, botones, el título
//! de la pestaña y minimizar / maximizar / cerrar. Arrastrarla mueve la ventana; doble clic maximiza.

use super::{Action, Orion};
use crate::colors::{surface, theme};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, ViewportCommand, vec2};

const HEIGHT: f32 = 34.0;
/// Ancho de cada botón de la ventana (como los de Windows 11).
const WINDOW_BUTTON: f32 = 46.0;

impl Orion {
    pub(super) fn title_bar(&self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        let frame = egui::Frame::new().fill(surface(theme().sidebar()));
        egui::Panel::top("barra")
            .exact_size(HEIGHT)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                let bar = ui.max_rect();
                let ctx = ui.ctx().clone();
                // Primero el fondo que se arrastra: los botones van encima y se llevan sus clics.
                let drag = ui.interact(
                    bar,
                    ui.scope_id().with("arrastrar"),
                    Sense::click_and_drag(),
                );
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                if drag.drag_started() {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
                if drag.double_clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                }

                // Logo: el ícono de Orion y el nombre.
                let painter = ui.painter();
                let mid = bar.center().y;
                let font = egui::FontId::proportional(13.0);
                let logo =
                    Rect::from_center_size(Pos2::new(bar.min.x + 20.0, mid), vec2(18.0, 18.0));
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(self.logo.id(), logo, uv, Color32::WHITE);
                painter.text(
                    Pos2::new(bar.min.x + 32.0, mid),
                    egui::Align2::LEFT_CENTER,
                    "Orion",
                    font.clone(),
                    theme().fg,
                );

                // Botones de Orion.
                let next_theme = (crate::colors::find_theme(&self.settings.theme).unwrap_or(0) + 1)
                    % crate::colors::THEMES.len();
                let buttons = [
                    ("⚙", "Configuración   Ctrl+,", Action::OpenSettings),
                    ("⌘", "Paleta de comandos   Ctrl+Shift+P", Action::Palette),
                    (
                        "◫",
                        "Dividir a la derecha   Ctrl+Shift+D",
                        Action::Split(true),
                    ),
                    ("◐", "Cambiar de tema", Action::Theme(next_theme)),
                ];
                let mut x = bar.min.x + 86.0;
                for (icon, tip, act) in buttons {
                    let r = Rect::from_center_size(Pos2::new(x, mid), vec2(28.0, 26.0));
                    let button = ui.interact(r, ui.scope_id().with(icon), Sense::click());
                    let color = if button.hovered() {
                        ui.painter().rect_filled(r, 6.0, theme().hover());
                        theme().fg
                    } else {
                        theme().ansi[7]
                    };
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        icon,
                        egui::FontId::proportional(15.0),
                        color,
                    );
                    if button.on_hover_text(tip).clicked() {
                        action = Some(act);
                    }
                    x += 32.0;
                }

                // Título de la pestaña, al medio, sin pisar los botones.
                let title = self
                    .tabs
                    .get(self.active)
                    .map(|t| t.focused().title())
                    .unwrap_or_default();
                let left = x;
                let right = bar.max.x - 3.0 * WINDOW_BUTTON - 8.0;
                let galley = ui.painter().layout_no_wrap(title, font, theme().ansi[7]);
                let center = bar.center().x.clamp(
                    left + galley.size().x / 2.0,
                    (right - galley.size().x / 2.0).max(left),
                );
                let clip = Rect::from_x_y_ranges(left..=right.max(left), bar.y_range());
                ui.painter().with_clip_rect(clip).galley(
                    Pos2::new(center - galley.size().x / 2.0, mid - galley.size().y / 2.0),
                    galley,
                    theme().ansi[7],
                );

                // Minimizar, maximizar y cerrar: dibujados con líneas, nítidos en cualquier escala.
                let stroke = |hovered: bool| {
                    Stroke::new(1.0, if hovered { theme().fg } else { theme().ansi[7] })
                };
                let window_button = |ui: &egui::Ui, i: f32, id: &str| {
                    let r = Rect::from_min_size(
                        Pos2::new(bar.max.x - (3.0 - i) * WINDOW_BUTTON, bar.min.y),
                        vec2(WINDOW_BUTTON, HEIGHT),
                    );
                    (r, ui.interact(r, ui.scope_id().with(id), Sense::click()))
                };
                let (r, minimize) = window_button(ui, 0.0, "minimizar");
                if minimize.hovered() {
                    ui.painter().rect_filled(r, 0.0, theme().hover());
                }
                ui.painter().hline(
                    r.center().x - 5.0..=r.center().x + 5.0,
                    r.center().y,
                    stroke(minimize.hovered()),
                );
                if minimize.clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                }

                let (r, maximize) = window_button(ui, 1.0, "maximizar");
                if maximize.hovered() {
                    ui.painter().rect_filled(r, 0.0, theme().hover());
                }
                let square = Rect::from_center_size(r.center(), vec2(10.0, 10.0));
                if maximized {
                    // Restaurar: dos cuadrados encimados.
                    ui.painter().rect_stroke(
                        square.translate(vec2(2.0, -2.0)),
                        1.0,
                        stroke(maximize.hovered()),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().rect_filled(
                        square.translate(vec2(-1.0, 1.0)),
                        1.0,
                        surface(theme().sidebar()),
                    );
                    ui.painter().rect_stroke(
                        square.translate(vec2(-1.0, 1.0)),
                        1.0,
                        stroke(maximize.hovered()),
                        egui::StrokeKind::Inside,
                    );
                } else {
                    ui.painter().rect_stroke(
                        square,
                        1.0,
                        stroke(maximize.hovered()),
                        egui::StrokeKind::Inside,
                    );
                }
                let tip = if maximized { "Restaurar" } else { "Maximizar" };
                if maximize.on_hover_text(tip).clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                }

                let (r, close) = window_button(ui, 2.0, "cerrar");
                let red = Color32::from_rgb(0xc4, 0x2b, 0x1c); // el rojo de cerrar de Windows 11
                if close.hovered() {
                    ui.painter().rect_filled(r, 0.0, red);
                }
                let x_stroke = Stroke::new(
                    1.0,
                    if close.hovered() {
                        Color32::WHITE
                    } else {
                        theme().ansi[7]
                    },
                );
                let c = r.center();
                ui.painter()
                    .line_segment([c - vec2(5.0, 5.0), c + vec2(5.0, 5.0)], x_stroke);
                ui.painter()
                    .line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], x_stroke);
                if close.clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            });
        action
    }
}
