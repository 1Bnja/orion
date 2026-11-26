//! Empty home: the logo, the name and the quick actions with their shortcuts, shown when no
//! terminal is open. Every color comes from the theme.

use super::chrome::{Icon, draw_icon_size, label, medium};
use super::overlay::mix;
use super::{Action, Orion};
use crate::colors::{surface, theme};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, StrokeKind,
    TextureHandle, pos2, vec2,
};

/// One quick action shown on the home screen.
struct Shortcut {
    label: &'static str,
    detail: &'static str,
    icon: Icon,
    keys: &'static str,
    action: Action,
}

fn shortcuts() -> [Shortcut; 5] {
    [
        Shortcut {
            label: "Nueva pestaña",
            detail: "Abre una terminal",
            icon: Icon::Plus,
            keys: keys!("+Shift+T"),
            action: Action::NewTab(None),
        },
        Shortcut {
            label: "Nuevo proyecto",
            detail: "Elige un repo y quién trabajará en él",
            icon: Icon::Folder,
            keys: keys!("+Shift+O"),
            action: Action::NewProject,
        },
        Shortcut {
            label: "Paleta de comandos",
            detail: "Busca cualquier acción",
            icon: Icon::Search,
            keys: keys!("+Shift+P"),
            action: Action::Palette,
        },
        Shortcut {
            label: "Barra lateral",
            detail: "Muestra u oculta las pestañas",
            icon: Icon::Sidebar,
            keys: keys!("+B"),
            action: Action::ToggleSidebar,
        },
        Shortcut {
            label: "Configuración",
            detail: "Tema, fuente, shell y más",
            icon: Icon::Gear,
            keys: keys!("+,"),
            action: Action::OpenSettings,
        },
    ]
}

/// One key as it is drawn in its own cap: on macOS the usual glyphs, elsewhere the word.
fn key_cap(part: &str) -> &str {
    if cfg!(target_os = "macos") {
        match part {
            "Cmd" => "⌘",
            "Shift" => "⇧",
            "Alt" | "Option" => "⌥",
            "Ctrl" => "⌃",
            other => other,
        }
    } else {
        part
    }
}

/// Size of the key caps and the gap between them.
const CAP_H: f32 = 24.0;
const CAP_GAP: f32 = 4.0;
const CAP_PAD: f32 = 8.0;
const ROW_H: f32 = 58.0;
const ROW_GAP: f32 = 8.0;
const MAX_WIDTH: f32 = 560.0;
/// The square behind the logo, and the cat inside it.
const TILE: f32 = 112.0;
const MARK: f32 = 78.0;
/// Logo, name and tagline: what goes above the actions.
const HERO_H: f32 = TILE + 108.0;

const LOGO_SVG: &[u8] = include_bytes!("../../assets/logo.svg");

/// The SVG drawn at exactly `side` × `side` pixels, in white so it can be tinted.
fn rasterize(svg: &[u8], side: u32) -> egui::ColorImage {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).expect("logo SVG");
    let mut pixels = tiny_skia::Pixmap::new(side, side).expect("logo size");
    let scale = side as f32 / tree.size().width();
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixels.as_mut());
    let size = [side as usize, side as usize];
    egui::ColorImage::from_rgba_premultiplied(size, pixels.data())
}

/// The logo for the current screen scale: rendered once per pixel size, so it is never stretched.
fn logo_texture(ctx: &egui::Context, side: u32) -> TextureHandle {
    let id = egui::Id::unique(("logo-svg", side));
    if let Some(handle) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return handle;
    }
    let image = rasterize(LOGO_SVG, side);
    let handle = ctx.load_texture("logo-svg", image, Default::default());
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    handle
}

impl Orion {
    /// Full-pane home when every tab is closed.
    pub(super) fn draw_home(&mut self, ui: &mut egui::Ui, area: egui::Rect) -> Option<Action> {
        let frame = surface(theme().sidebar());
        ui.painter().rect_filled(area, 0.0, frame);
        let pad = egui::Vec2::splat(super::PANE_GAP);
        let inner = egui::Rect::from_min_max(area.min + pad, area.max - pad);
        ui.painter()
            .rect_filled(inner, super::PANE_RADIUS, surface(theme().bg));
        ui.painter().rect_stroke(
            inner,
            super::PANE_RADIUS,
            Stroke::new(1.0, theme().line()),
            StrokeKind::Inside,
        );
        paint_home(ui, inner)
    }
}

/// A step from the background towards the text color.
fn tone(strength: f32) -> Color32 {
    mix(theme().bg, theme().fg, strength)
}

fn paint_home(ui: &mut egui::Ui, rect: Rect) -> Option<Action> {
    let list = shortcuts();
    let list_h = list.len() as f32 * (ROW_H + ROW_GAP) - ROW_GAP;
    // In a short pane the actions matter more than the logo.
    let hero = rect.height() >= HERO_H + list_h + 32.0;
    let block_h = list_h + if hero { HERO_H } else { 0.0 };
    let mut y = (rect.center().y - block_h / 2.0).max(rect.min.y + 12.0);
    let painter = ui.painter().with_clip_rect(rect);

    if hero {
        let tile = Rect::from_center_size(pos2(rect.center().x, y + TILE / 2.0), vec2(TILE, TILE));
        painter.rect(
            tile,
            28.0,
            tone(0.07),
            Stroke::new(1.0, tone(0.18)),
            StrokeKind::Inside,
        );
        let scale = ui.ctx().pixels_per_point();
        let side = (MARK * scale).round();
        let texture = logo_texture(ui.ctx(), side as u32);
        // On whole pixels, one texel per pixel: the edges stay sharp.
        let corner = tile.center() - vec2(side, side) / (2.0 * scale);
        let corner = pos2(
            (corner.x * scale).round() / scale,
            (corner.y * scale).round() / scale,
        );
        let mark = Rect::from_min_size(corner, vec2(side, side) / scale);
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        painter.image(texture.id(), mark, uv, theme().fg);

        painter.text(
            pos2(rect.center().x, tile.max.y + 34.0),
            Align2::CENTER_CENTER,
            "Orion",
            medium(30.0),
            theme().fg,
        );
        painter.text(
            pos2(rect.center().x, tile.max.y + 64.0),
            Align2::CENTER_CENTER,
            "Tus terminales y agentes, en un solo lugar",
            FontId::proportional(15.0),
            tone(0.68),
        );
        y += HERO_H;
    }

    let width = MAX_WIDTH.min(rect.width() - 32.0);
    let mut action = None;
    for item in list {
        let row = Rect::from_min_size(pos2(rect.center().x - width / 2.0, y), vec2(width, ROW_H));
        let id = ui.scope_id().with(("home", item.label));
        let response = ui.interact(row, id, Sense::click());
        let hover = ui
            .ctx()
            .animate_bool_with_time(id.with("hover"), response.hovered(), 0.06);
        let down = response.is_pointer_button_down_on();
        let body = if down { row.shrink(1.0) } else { row };
        painter.rect(
            body,
            12.0,
            tone(0.025 + 0.045 * hover),
            Stroke::new(1.0, tone(0.13 + 0.17 * hover)),
            StrokeKind::Inside,
        );

        let icon =
            Rect::from_center_size(pos2(body.min.x + 30.0, body.center().y), vec2(36.0, 36.0));
        painter.rect_filled(icon, 9.0, tone(0.08 + 0.05 * hover));
        draw_icon_size(ui, item.icon, icon.center(), theme().fg, 18.0);

        // Caps from the right edge inwards, so the last key sits at the end of the row.
        let mut x = body.max.x - 14.0;
        for part in item.keys.rsplit('+') {
            let galley = painter.layout_no_wrap(key_cap(part).into(), medium(12.5), theme().fg);
            let cap_w = (galley.size().x + CAP_PAD * 2.0).max(CAP_H);
            let cap = Rect::from_min_max(
                pos2(x - cap_w, body.center().y - CAP_H / 2.0),
                pos2(x, body.center().y + CAP_H / 2.0),
            );
            painter.rect(
                cap,
                6.0,
                tone(0.06),
                Stroke::new(1.0, tone(0.22)),
                StrokeKind::Inside,
            );
            painter.galley(cap.center() - galley.size() / 2.0, galley, theme().fg);
            x -= cap_w + CAP_GAP;
        }

        let line = |offset: f32| {
            Rect::from_min_max(
                pos2(icon.max.x + 14.0, body.center().y + offset - 10.0),
                pos2(x - 8.0, body.center().y + offset + 10.0),
            )
        };
        label(&painter, line(-9.0), item.label, medium(15.0), theme().fg);
        label(
            &painter,
            line(10.0),
            item.detail,
            FontId::proportional(12.5),
            tone(0.62),
        );

        if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            action = Some(item.action);
        }
        y += ROW_H + ROW_GAP;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_svg_has_a_face_with_cut_out_eyes() {
        let side = 168; // dos píxeles por unidad del diseño
        let image = rasterize(LOGO_SVG, side);
        // Un punto en las unidades del viewBox ("18 16 84 84").
        let alpha = |x: usize, y: usize| image.pixels[(y - 16) * 2 * 168 + (x - 18) * 2].a();
        assert_eq!(alpha(60, 80), 255, "la cara está pintada");
        assert_eq!(alpha(60, 20), 0, "entre las orejas no hay nada");
        assert_eq!(alpha(74, 56), 0, "el ojo cerrado es un recorte");
        assert_eq!(alpha(49, 59), 0, "el ojo de prompt es un recorte");
    }
}
