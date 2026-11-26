//! La base común de las ventanas flotantes (paleta y orquestador): el fondo atenuado, el panel con su
//! sombra y su entrada animada, y las piezas con sus estados (botones, chips, teclas).

use super::chrome::{Icon, draw_icon_size, label, medium};
use crate::colors::theme;
use eframe::egui::emath::TSTransform;
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Painter, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, pos2, vec2,
};

const OPEN_SECONDS: f32 = 0.11;
const CLOSE_SECONDS: f32 = 0.07;
/// Lo que tarda un botón en encenderse bajo el mouse.
const HOVER_SECONDS: f32 = 0.06;
pub(super) const PANEL_RADIUS: u8 = 14;

/// `a` llevado hacia `b` en una fracción `t`.
pub(super) fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t)
}

/// El fondo del panel: un escalón sobre el de la terminal, siempre opaco.
pub(super) fn panel_fill() -> Color32 {
    let t = theme();
    if t.is_light() {
        t.bg
    } else {
        mix(t.bg, t.fg, 0.035)
    }
}

/// Un gris sobre el panel: `strength` = cuánto se acerca al color del texto.
pub(super) fn tint(strength: f32) -> Color32 {
    mix(panel_fill(), theme().fg, strength)
}

pub(super) fn border() -> Color32 {
    tint(0.11)
}

/// Texto de apoyo (descripciones): legible, pero claramente por debajo del principal.
pub(super) fn dim_text() -> Color32 {
    mix(
        theme().fg,
        panel_fill(),
        if theme().is_light() { 0.22 } else { 0.32 },
    )
}

/// Texto casi decorativo: títulos de grupo, pistas.
pub(super) fn faint_text() -> Color32 {
    mix(
        theme().fg,
        panel_fill(),
        if theme().is_light() { 0.34 } else { 0.46 },
    )
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// El avance de la entrada (0 = cerrado, 1 = abierto).
fn progress(ctx: &egui::Context, id: egui::Id, open: bool) -> f32 {
    let seconds = if open { OPEN_SECONDS } else { CLOSE_SECONDS };
    ctx.animate_bool_with_time(id.with("visible"), open, seconds)
}

/// Si hay que dibujar el panel: abierto, o todavía saliendo. Hay que preguntarlo en cada frame:
/// así la primera entrada también se anima.
pub(super) fn visible(ctx: &egui::Context, id: &str, open: bool) -> bool {
    progress(ctx, egui::Id::unique(id), open) > 0.0 || open
}

pub(super) struct Shown {
    /// Se hizo clic en el fondo atenuado.
    pub(super) clicked_outside: bool,
}

/// Dibuja un panel flotante centrado. Hay que llamarla en cada frame, también cerrada: así la
/// entrada y la salida se animan. `top` es la distancia al borde de arriba, como fracción del alto.
pub(super) fn show<R>(
    ctx: &egui::Context,
    id: &str,
    open: bool,
    width: f32,
    top: f32,
    content: impl FnOnce(&mut Ui) -> R,
) -> Option<Shown> {
    let id = egui::Id::unique(id);
    let t = progress(ctx, id, open);
    if !open && t <= 0.0 {
        return None;
    }
    let eased = ease_out(t);
    let screen = ctx.content_rect();
    let width = width.min(screen.width() - 32.0);

    let scrim = egui::Area::new(id.with("scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .fade_in(false)
        .interactable(open)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(screen.size(), Sense::click());
            let strength = if theme().is_light() { 60.0 } else { 130.0 };
            let alpha = (strength * eased) as u8;
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_black_alpha(alpha));
            response.clicked()
        });

    let y = screen.min.y + (screen.height() * top).max(24.0);
    let left = pos2(screen.center().x - width / 2.0, y);
    let panel = egui::Area::new(id.with("panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(left.round())
        .constrain(false)
        .fade_in(false)
        // Inmóvil: así el Area detecta clics y bloquea al scrim. Si se pudiera arrastrar, egui le
        // da solo `Sense::DRAG` y los clics en textos, márgenes y huecos llegarían al fondo y
        // cerrarían la ventana.
        .movable(false)
        .interactable(open)
        .show(ctx, |ui| {
            ui.set_opacity(eased);
            let shadow = egui::epaint::Shadow {
                offset: [0, 18],
                blur: 60,
                spread: 4,
                color: Color32::from_black_alpha(if theme().is_light() { 50 } else { 120 }),
            };
            egui::Frame::new()
                .fill(panel_fill())
                .stroke(Stroke::new(1.0, border()))
                .corner_radius(PANEL_RADIUS)
                .shadow(shadow)
                .show(ui, |ui| {
                    ui.set_width(width - 2.0);
                    ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
                    content(ui)
                })
                .inner
        });

    // El fondo recién abierto va arriba de todo, y el panel siempre encima de su fondo. Solo mientras
    // entra: después los menús del panel tienen que poder quedar por encima.
    ctx.set_sublayer(scrim.response.layer_id, panel.response.layer_id);
    if open && t < 1.0 {
        ctx.move_to_top(scrim.response.layer_id);
    }
    // Entra creciendo un poco desde arriba.
    let scale = 0.98 + 0.02 * eased;
    let pivot = pos2(screen.center().x, y);
    let shift = pivot.to_vec2() * (1.0 - scale) + vec2(0.0, -6.0 * (1.0 - eased));
    let transform = if t >= 1.0 {
        TSTransform::IDENTITY
    } else {
        TSTransform::new(shift, scale)
    };
    ctx.set_transform_layer(panel.response.layer_id, transform);

    Some(Shown {
        clicked_outside: open && scrim.inner,
    })
}

/// Una línea separadora de lado a lado.
pub(super) fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, border());
}

const KEY_HEIGHT: f32 = 19.0;

fn key_galleys(ui: &Ui, keys: &str, color: Color32) -> Vec<std::sync::Arc<egui::Galley>> {
    keys.split('+')
        .filter(|k| !k.is_empty())
        .map(|k| {
            ui.painter()
                .layout_no_wrap(k.to_string(), medium(11.0), color)
        })
        .collect()
}

/// El ancho que ocupa un atajo dibujado como teclas.
pub(super) fn keys_width(ui: &Ui, keys: &str) -> f32 {
    let galleys = key_galleys(ui, keys, theme().fg);
    let caps: f32 = galleys
        .iter()
        .map(|g| (g.size().x + 10.0).max(KEY_HEIGHT))
        .sum();
    caps + 3.0 * galleys.len().saturating_sub(1) as f32
}

/// Dibuja "Ctrl+Shift+T" como teclas sueltas, alineadas a la derecha de `right`.
pub(super) fn keys_at(ui: &Ui, right: Pos2, keys: &str, color: Color32) {
    let mut x = right.x - keys_width(ui, keys);
    for galley in key_galleys(ui, keys, color) {
        let width = (galley.size().x + 10.0).max(KEY_HEIGHT);
        let cap = Rect::from_min_size(pos2(x, right.y - KEY_HEIGHT / 2.0), vec2(width, KEY_HEIGHT));
        ui.painter().rect(
            cap,
            5.0,
            tint(0.055),
            Stroke::new(1.0, tint(0.13)),
            StrokeKind::Inside,
        );
        let pos = cap.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, color);
        x += width + 3.0;
    }
}

/// Teclas como un widget más, para filas con `ui.horizontal`.
pub(super) fn keys(ui: &mut Ui, text: &str) {
    let size = vec2(keys_width(ui, text), KEY_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    keys_at(ui, rect.right_center(), text, dim_text());
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    /// La acción principal: relleno con el color del texto.
    Primary,
    Secondary,
    /// Sin fondo hasta que el mouse lo toca.
    Ghost,
    /// Como `Ghost`, pero se pone rojo: borra u olvida algo.
    Danger,
}

/// Un botón con sus estados: normal, bajo el mouse, apretado, con foco y desactivado.
pub(super) struct Button<'a> {
    text: &'a str,
    kind: Kind,
    icon: Option<Icon>,
    keys: &'a str,
    enabled: bool,
    small: bool,
}

impl<'a> Button<'a> {
    pub(super) fn new(text: &'a str, kind: Kind) -> Self {
        Self {
            text,
            kind,
            icon: None,
            keys: "",
            enabled: true,
            small: false,
        }
    }

    pub(super) fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub(super) fn keys(mut self, keys: &'a str) -> Self {
        self.keys = keys;
        self
    }

    pub(super) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub(super) fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub(super) fn show(self, ui: &mut Ui) -> Response {
        let (height, pad, size) = if self.small {
            (26.0, 10.0, 12.0)
        } else {
            (34.0, 14.0, 13.0)
        };
        let galley = ui
            .painter()
            .layout_no_wrap(self.text.to_string(), medium(size), theme().fg);
        let icon = if self.icon.is_some() { 21.0 } else { 0.0 };
        // En el botón principal las teclas van como texto: las cajitas no se leen sobre el relleno.
        let hint =
            ui.painter()
                .layout_no_wrap(self.keys.replace('+', " "), medium(11.0), theme().fg);
        let hint_width = if self.keys.is_empty() {
            0.0
        } else {
            hint.size().x + 10.0
        };
        let width = pad * 2.0 + icon + galley.size().x + hint_width;
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);

        let hovered = self.enabled && response.hovered();
        let down = self.enabled && response.is_pointer_button_down_on();
        let hover =
            ui.ctx()
                .animate_bool_with_time(response.id.with("hover"), hovered, HOVER_SECONDS);
        let t = theme();
        let danger = t.ansi[1];
        let (mut fill, mut stroke, mut text) = match self.kind {
            Kind::Primary => (
                mix(t.fg, panel_fill(), 0.08 * (1.0 - hover)),
                Color32::TRANSPARENT,
                panel_fill(),
            ),
            Kind::Secondary => (tint(0.07 + 0.05 * hover), tint(0.14 + 0.08 * hover), t.fg),
            Kind::Ghost => (
                tint(0.08).gamma_multiply(hover),
                Color32::TRANSPARENT,
                mix(dim_text(), t.fg, hover),
            ),
            Kind::Danger => (
                mix(panel_fill(), danger, 0.16).gamma_multiply(hover),
                Color32::TRANSPARENT,
                mix(dim_text(), danger, hover),
            ),
        };
        if down {
            fill = mix(fill, panel_fill(), 0.18);
        }
        if !self.enabled {
            fill = tint(0.04);
            stroke = tint(0.08);
            text = mix(t.fg, panel_fill(), 0.65);
        }
        let body = if down { rect.shrink(1.0) } else { rect };
        let radius = if self.small { 7.0 } else { 9.0 };
        ui.painter().rect(
            body,
            radius,
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Inside,
        );
        if response.has_focus() {
            ui.painter().rect_stroke(
                body.expand(2.0),
                radius + 2.0,
                Stroke::new(1.5, tint(0.55)),
                StrokeKind::Outside,
            );
        }
        let mut x = body.center().x - (width - pad * 2.0) / 2.0;
        if let Some(i) = self.icon {
            draw_icon_size(ui, i, pos2(x + 7.0, body.center().y), text, 14.0);
            x += icon;
        }
        let text_width = galley.size().x;
        ui.painter().galley_with_override_text_color(
            pos2(x, body.center().y - galley.size().y / 2.0),
            galley,
            text,
        );
        if !self.keys.is_empty() {
            let pos = pos2(x + text_width + 10.0, body.center().y - hint.size().y / 2.0);
            ui.painter()
                .galley_with_override_text_color(pos, hint, text.gamma_multiply(0.6));
        }
        response.on_hover_cursor(if self.enabled {
            CursorIcon::PointingHand
        } else {
            CursorIcon::NotAllowed
        })
    }
}

/// Un botón cuadrado de solo un ícono.
pub(super) fn icon_button(ui: &mut Ui, icon: Icon, tip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        response.hovered(),
        HOVER_SECONDS,
    );
    ui.painter()
        .rect_filled(rect, 7.0, tint(0.09).gamma_multiply(hover));
    let color = mix(dim_text(), theme().fg, hover);
    draw_icon_size(ui, icon, rect.center(), color, 15.0);
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tip)
}

/// Una pastilla que se puede elegir: filtros, ejecutores, planificador.
pub(super) fn chip(ui: &mut Ui, text: &str, on: bool, enabled: bool, check: bool) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), medium(12.0), theme().fg);
    let mark = if check { 16.0 } else { 0.0 };
    let size = vec2(galley.size().x + 20.0 + mark, 26.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let hovered = enabled && response.hovered();
    let hover = ui
        .ctx()
        .animate_bool_with_time(response.id.with("hover"), hovered, HOVER_SECONDS);
    let selected = ui
        .ctx()
        .animate_bool_with_time(response.id.with("on"), on, HOVER_SECONDS);
    let fill = tint(0.12 * selected + 0.06 * hover);
    let stroke = tint(0.11 + 0.13 * selected + 0.06 * hover);
    let color = if !enabled {
        mix(theme().fg, panel_fill(), 0.68)
    } else {
        mix(dim_text(), theme().fg, selected.max(hover))
    };
    ui.painter().rect(
        rect,
        13.0,
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    if check {
        let c = pos2(rect.min.x + 15.0, rect.center().y);
        if on {
            draw_icon_size(ui, Icon::Check, c, theme().fg, 13.0);
        } else {
            ui.painter().circle_stroke(c, 4.0, Stroke::new(1.0, color));
        }
    }
    let pos = pos2(
        rect.min.x + 10.0 + mark,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter()
        .galley_with_override_text_color(pos, galley, color);
    response.on_hover_cursor(if enabled {
        CursorIcon::PointingHand
    } else {
        CursorIcon::NotAllowed
    })
}

/// Una etiqueta chica que no se puede tocar: un estado, una rama.
pub(super) fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), medium(11.0), color);
    let size = vec2(galley.size().x + 14.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, 6.0, mix(panel_fill(), color, 0.14));
    let pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(pos, galley, color);
    response
}

/// Texto de una línea centrado en vertical, que termina en "…" si no entra.
pub(super) fn line_at(painter: &Painter, rect: Rect, text: &str, font: FontId, color: Color32) {
    label(painter, rect, text, font, color);
}

/// Texto centrado en `center`.
pub(super) fn centered(painter: &Painter, center: Pos2, text: &str, font: FontId, color: Color32) {
    painter.text(center, Align2::CENTER_CENTER, text, font, color);
}
