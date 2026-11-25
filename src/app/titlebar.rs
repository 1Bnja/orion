//! The title bar. Orion draws its own instead of the system one: the logo and the name on the
//! left, the folder of the focused pane in the middle and Orion's buttons on the right.
//! On Windows it also draws minimize / maximize / close. On Mac the three circles are the native
//! ones: the bar only leaves room for them on the left.
//! Dragging the bar moves the window; a double click maximizes it.

use super::chrome::{Icon, icon_button, medium};
use super::{Action, MAC, Orion};
use crate::colors::{surface, theme};
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, ViewportCommand, vec2};
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

const HEIGHT: f32 = 34.0;
/// Width of each window button (like the ones in Windows 11).
const WINDOW_BUTTON: f32 = 46.0;
/// Mac: room on the left for the native circles.
const LIGHTS_WIDTH: f32 = 72.0;
const BUTTON: egui::Vec2 = vec2(28.0, 24.0);
const BUTTON_GAP: f32 = 2.0;

/// The folder as (parents, name), with the home folder written as "~":
/// "/home/ana/code/orion" is ("~/code/", "orion").
fn breadcrumb(cwd: &Path, home: Option<&Path>) -> (String, String) {
    let text = match home.and_then(|home| cwd.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~{MAIN_SEPARATOR}{}", rest.display()),
        None => cwd.display().to_string(),
    };
    match text.rfind(MAIN_SEPARATOR) {
        Some(i) if i + 1 < text.len() => (text[..=i].to_string(), text[i + 1..].to_string()),
        _ => (String::new(), text),
    }
}

/// `parents` without its first folder, with "…" in its place: "~/code/rust/" becomes
/// "…/rust/". None when there is nothing left to drop.
fn drop_first(parents: &str) -> Option<String> {
    // "~" and "…" both stand for a folder already dropped; skip them the same way.
    let rest = parents
        .strip_prefix('…')
        .or_else(|| parents.strip_prefix('~'))
        .unwrap_or(parents);
    let rest = rest.trim_start_matches(MAIN_SEPARATOR);
    let cut = rest.find(MAIN_SEPARATOR)?;
    Some(format!("…{MAIN_SEPARATOR}{}", &rest[cut + 1..]))
}

fn home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(PathBuf::from)
}

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
                // The draggable background goes first: the buttons sit on top and take their clicks.
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
                let mid = bar.center().y;

                // Left: the logo and the name.
                let left = bar.min.x + if MAC { LIGHTS_WIDTH + 8.0 } else { 12.0 };
                let logo = Rect::from_min_size(Pos2::new(left, mid - 8.0), vec2(16.0, 16.0));
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                ui.painter().image(self.logo.id(), logo, uv, theme().fg);
                let name = ui.painter().text(
                    Pos2::new(logo.max.x + 7.0, mid),
                    egui::Align2::LEFT_CENTER,
                    "Orion",
                    medium(13.0),
                    theme().fg,
                );

                // Right: Orion's buttons (before the window buttons on Windows).
                let buttons = [
                    (
                        Icon::Sidebar,
                        keys!("Mostrar/ocultar barra lateral   ", "+B", ""),
                        Action::ToggleSidebar,
                    ),
                    (
                        Icon::Split,
                        keys!("Dividir a la derecha   ", "+Shift+D", ""),
                        Action::Split(true),
                    ),
                    (
                        Icon::Theme,
                        "Siguiente tema",
                        Action::Theme(self.next_theme()),
                    ),
                    (
                        Icon::Palette,
                        keys!("Paleta de comandos   ", "+Shift+P", ""),
                        Action::Palette,
                    ),
                    (
                        Icon::Gear,
                        keys!("Configuración   ", "+,", ""),
                        Action::OpenSettings,
                    ),
                ];
                let right = bar.max.x - if MAC { 8.0 } else { 3.0 * WINDOW_BUTTON + 6.0 };
                let buttons_left = right - buttons.len() as f32 * (BUTTON.x + BUTTON_GAP);
                let mut x = buttons_left;
                for (icon, tip, act) in buttons {
                    let rect = Rect::from_min_size(Pos2::new(x, mid - BUTTON.y / 2.0), BUTTON);
                    let button = icon_button(ui, rect, ui.scope_id().with(tip), icon);
                    if button.on_hover_text(tip).clicked() {
                        action = Some(act);
                    }
                    x += BUTTON.x + BUTTON_GAP;
                }

                // Middle: where the focused pane is.
                let space =
                    Rect::from_x_y_ranges(name.max.x + 16.0..=buttons_left - 16.0, bar.y_range());
                self.draw_location(ui, space, bar.center().x);

                if !MAC {
                    windows_buttons(ui, bar, maximized);
                }
            });
        action
    }

    /// The folder of the focused pane, centered on `center` when it fits in `space`: the parent
    /// folders in a quiet color and the last one in the text color. If the shell did not report
    /// a folder, the title of the pane.
    fn draw_location(&self, ui: &egui::Ui, space: Rect, center: f32) {
        let Some(session) = self.tabs.get(self.active).map(|tab| tab.focused()) else {
            return;
        };
        let (mut parents, name) = match &session.cwd {
            Some(cwd) => breadcrumb(cwd, home().as_deref()),
            None => (String::new(), session.title()),
        };
        let layout = |parents: &str| {
            let quiet = TextFormat::simple(FontId::proportional(12.5), theme().muted());
            let mut job = LayoutJob::default();
            job.append(parents, 0.0, quiet);
            job.append(&name, 0.0, TextFormat::simple(medium(12.5), theme().fg));
            ui.painter().layout_job(job)
        };
        let mut galley = layout(&parents);
        // Too long: drop folders from the left until it fits.
        while galley.size().x > space.width()
            && let Some(shorter) = drop_first(&parents)
        {
            parents = shorter;
            galley = layout(&parents);
        }
        let half = galley.size().x / 2.0;
        let x = center.clamp(
            space.min.x + half,
            (space.max.x - half).max(space.min.x + half),
        );
        let pos = Pos2::new(x - half, space.center().y - galley.size().y / 2.0);
        ui.painter()
            .with_clip_rect(space)
            .galley(pos, galley, theme().fg);
    }
}

/// Windows: minimize, maximize and close on the right, drawn with lines (sharp at any scale).
fn windows_buttons(ui: &egui::Ui, bar: Rect, maximized: bool) {
    let ctx = ui.ctx();
    let stroke = |hovered: bool| {
        let color = if hovered {
            theme().fg
        } else {
            theme().secondary()
        };
        Stroke::new(1.0, color)
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
        // Restore: two overlapping squares.
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
    let red = Color32::from_rgb(0xc4, 0x2b, 0x1c); // the close red of Windows 11
    if close.hovered() {
        ui.painter().rect_filled(r, 0.0, red);
    }
    let x_stroke = Stroke::new(
        1.0,
        if close.hovered() {
            Color32::WHITE
        } else {
            theme().secondary()
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path written with "/" in the form of this system.
    fn native(path: &str) -> String {
        path.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

    #[test]
    fn breadcrumb_splits_the_folder_and_shortens_home() {
        let home = PathBuf::from(native("/home/ana"));
        let crumb = |path: &str| breadcrumb(Path::new(&native(path)), Some(&home));
        assert_eq!(
            crumb("/home/ana/code/orion"),
            (native("~/code/"), "orion".to_string())
        );
        assert_eq!(crumb("/home/ana"), (String::new(), "~".to_string()));
        assert_eq!(crumb("/usr/local"), (native("/usr/"), "local".to_string()));
        // Another user's folder only starts the same: it is not inside home.
        assert_eq!(crumb("/home/anabel"), (native("/home/"), "anabel".into()));
        let root = breadcrumb(Path::new(&native("/")), None);
        assert_eq!(root, (String::new(), native("/")));
    }

    #[test]
    fn long_paths_lose_folders_from_the_left() {
        let step = |parents: &str| drop_first(&native(parents));
        assert_eq!(step("~/code/rust/"), Some(native("…/rust/")));
        assert_eq!(step("/usr/local/"), Some(native("…/local/")));
        assert_eq!(step("…/rust/"), Some(native("…/")));
        assert_eq!(step("…/"), None);
        assert_eq!(step(""), None);
    }
}
