//! Small pieces shared by Orion's own UI (the title bar and the sidebar): icons and text.
//! Icons are Tabler outline PNGs (MIT), tinted to the theme color.

use crate::colors::theme;
use crate::fonts::medium_family;
use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{self, Color32, FontId, Painter, Pos2, Rect, Sense, TextureHandle, pos2, vec2};

/// Corner radius of rows and buttons.
pub(super) const RADIUS: f32 = 7.0;

/// The UI font with a bit more weight.
pub(super) fn medium(size: f32) -> FontId {
    FontId::new(size, medium_family())
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub(super) enum Icon {
    Gear = 0,
    /// Command palette (prompt).
    Palette = 1,
    Sidebar = 2,
    Split = 3,
    Theme = 4,
    Plus = 5,
    Chevron = 6,
    Close = 7,
    Search = 8,
    Appearance = 9,
    Colors = 10,
    Font = 11,
    Terminal = 12,
    Advanced = 13,
    Save = 14,
    ChevronRight = 15,
}

impl Icon {
    fn png(self) -> &'static [u8] {
        match self {
            Icon::Gear => include_bytes!("../../assets/icons/settings.png"),
            Icon::Palette => include_bytes!("../../assets/icons/terminal-2.png"),
            Icon::Sidebar => include_bytes!("../../assets/icons/layout-sidebar.png"),
            Icon::Split => include_bytes!("../../assets/icons/columns-2.png"),
            Icon::Theme => include_bytes!("../../assets/icons/moon.png"),
            Icon::Plus => include_bytes!("../../assets/icons/plus.png"),
            Icon::Chevron => include_bytes!("../../assets/icons/chevron-down.png"),
            Icon::Close => include_bytes!("../../assets/icons/x.png"),
            Icon::Search => include_bytes!("../../assets/icons/search.png"),
            Icon::Appearance => include_bytes!("../../assets/icons/sun.png"),
            Icon::Colors => include_bytes!("../../assets/icons/palette.png"),
            Icon::Font => include_bytes!("../../assets/icons/typography.png"),
            Icon::Terminal => include_bytes!("../../assets/icons/terminal-2.png"),
            Icon::Advanced => include_bytes!("../../assets/icons/adjustments.png"),
            Icon::Save => include_bytes!("../../assets/icons/brush.png"),
            Icon::ChevronRight => include_bytes!("../../assets/icons/chevron-right.png"),
        }
    }
}

/// Loads a Tabler icon once per egui context (white PNG, tinted when drawn).
fn texture(ctx: &egui::Context, icon: Icon) -> TextureHandle {
    let id = egui::Id::unique(("tabler", icon as u8));
    if let Some(handle) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return handle;
    }
    let image = image::load_from_memory(icon.png()).expect("Tabler PNG");
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
    let handle = ctx.load_texture(format!("tabler-{icon:?}"), color, Default::default());
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    handle
}

/// Draws a Tabler icon centered on `c`, tinted with `color`.
pub(super) fn draw_icon(ui: &egui::Ui, icon: Icon, c: Pos2, color: Color32) {
    draw_icon_size(ui, icon, c, color, 16.0);
}

/// Same as `draw_icon`, with an explicit side length in points.
pub(super) fn draw_icon_size(ui: &egui::Ui, icon: Icon, c: Pos2, color: Color32, size: f32) {
    let tex = texture(ui.ctx(), icon);
    let rect = Rect::from_center_size(c, vec2(size, size));
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    ui.painter().image(tex.id(), rect, uv, color);
}

/// A button that is only an icon. It lights up under the mouse.
pub(super) fn icon_button(ui: &egui::Ui, rect: Rect, id: egui::Id, icon: Icon) -> egui::Response {
    let response = ui.interact(rect, id, Sense::click());
    let color = if response.hovered() {
        ui.painter()
            .rect_filled(rect, RADIUS - 1.0, theme().hover());
        theme().fg
    } else {
        theme().muted()
    };
    draw_icon(ui, icon, rect.center(), color);
    response
}

/// The parts of `rect` that `hole` does not cover, as up to four rectangles.
fn subtract(rect: Rect, hole: Rect) -> Vec<Rect> {
    let hole = hole.intersect(rect);
    if !hole.is_positive() {
        return vec![rect];
    }
    let pieces = [
        Rect::from_min_max(rect.min, pos2(rect.max.x, hole.min.y)),
        Rect::from_min_max(pos2(rect.min.x, hole.max.y), rect.max),
        Rect::from_min_max(pos2(rect.min.x, hole.min.y), hole.left_bottom()),
        Rect::from_min_max(hole.right_top(), pos2(rect.max.x, hole.max.y)),
    ];
    pieces.into_iter().filter(Rect::is_positive).collect()
}

/// Fills `area` with `color`, except the `holes`, which have rounded corners.
///
/// Painting the whole area and the holes' content on top would be simpler, but with a
/// see-through window (Mica, acrylic) the two layers would add up and look darker.
pub(super) fn paint_around(
    painter: &Painter,
    area: Rect,
    holes: impl IntoIterator<Item = Rect>,
    radius: f32,
    color: Color32,
) {
    let mut mesh = egui::Mesh::default();
    let mut pieces = vec![area];
    for hole in holes {
        pieces = pieces.iter().flat_map(|p| subtract(*p, hole)).collect();
        // Each corner: the bit of the hole's square corner that is outside its rounded one.
        for (corner, inward) in [
            (hole.left_top(), vec2(1.0, 1.0)),
            (hole.right_top(), vec2(-1.0, 1.0)),
            (hole.right_bottom(), vec2(-1.0, -1.0)),
            (hole.left_bottom(), vec2(1.0, -1.0)),
        ] {
            const STEPS: u32 = 8;
            let first = mesh.vertices.len() as u32;
            let center = corner + inward * radius;
            mesh.colored_vertex(corner, color);
            for step in 0..=STEPS {
                let angle = step as f32 / STEPS as f32 * std::f32::consts::FRAC_PI_2;
                let arc = vec2(-inward.x * angle.sin(), -inward.y * angle.cos()) * radius;
                mesh.colored_vertex(center + arc, color);
            }
            for step in 1..=STEPS {
                mesh.add_triangle(first, first + step, first + step + 1);
            }
        }
    }
    for piece in pieces {
        mesh.add_colored_rect(piece, color);
    }
    painter.add(mesh);
}

/// One line of text, left-aligned and vertically centered in `rect`. Text that does not fit
/// ends in "…". Returns the width it used.
pub(super) fn label(
    painter: &Painter,
    rect: Rect,
    text: impl Into<String>,
    font: FontId,
    color: Color32,
) -> f32 {
    let mut job = LayoutJob::simple_singleline(text.into(), font, color);
    job.wrap = TextWrapping::truncate_at_width(rect.width().max(0.0));
    let galley = painter.layout_job(job);
    let width = galley.size().x;
    let pos = pos2(rect.min.x, rect.center().y - galley.size().y / 2.0);
    painter.galley(pos, galley, color);
    width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtracting_a_hole_leaves_the_rest() {
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0));
        let area = |pieces: &[Rect]| pieces.iter().map(|r| r.area()).sum::<f32>();
        // A hole in the middle leaves four pieces around it.
        let hole = Rect::from_min_max(pos2(2.0, 2.0), pos2(6.0, 5.0));
        let pieces = subtract(rect, hole);
        assert_eq!(pieces.len(), 4);
        assert_eq!(area(&pieces), 100.0 - 12.0);
        assert!(pieces.iter().all(|p| !p.intersects(hole.shrink(0.1))));
        // A hole on an edge leaves fewer; one outside leaves the rectangle as it was.
        let corner = Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 10.0));
        assert_eq!(subtract(rect, corner).len(), 1);
        assert_eq!(subtract(rect, rect.translate(vec2(20.0, 0.0))), [rect]);
        assert!(subtract(rect, rect).is_empty());
    }

    #[test]
    fn every_tabler_png_loads() {
        for icon in [
            Icon::Gear,
            Icon::Palette,
            Icon::Sidebar,
            Icon::Split,
            Icon::Theme,
            Icon::Plus,
            Icon::Chevron,
            Icon::Close,
            Icon::Search,
            Icon::Appearance,
            Icon::Colors,
            Icon::Font,
            Icon::Terminal,
            Icon::Advanced,
            Icon::Save,
            Icon::ChevronRight,
        ] {
            let image = image::load_from_memory(icon.png()).expect("png");
            assert_eq!(image.width(), 48);
            assert_eq!(image.height(), 48);
        }
    }
}
