//! Empty home: a dim logo and a short list of shortcuts, shown when no terminal is open.

use super::chrome::medium;
use super::{Action, Orion};
use crate::colors::{surface, theme};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, vec2};

/// One quick action shown on the home screen.
struct Shortcut {
    label: &'static str,
    keys: &'static str,
    action: Action,
}

fn shortcuts() -> [Shortcut; 4] {
    [
        Shortcut {
            label: "Nueva pestaña",
            keys: keys!("+Shift+T"),
            action: Action::NewTab(None),
        },
        Shortcut {
            label: "Paleta de comandos",
            keys: keys!("+Shift+P"),
            action: Action::Palette,
        },
        Shortcut {
            label: "Barra lateral",
            keys: keys!("+B"),
            action: Action::ToggleSidebar,
        },
        Shortcut {
            label: "Configuración",
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
const CAP_H: f32 = 20.0;
const CAP_GAP: f32 = 4.0;
const CAP_PAD: f32 = 7.0;
const ROW_H: f32 = 32.0;

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
            egui::StrokeKind::Inside,
        );
        self.paint_home(ui, inner)
    }

    fn paint_home(&self, ui: &mut egui::Ui, rect: egui::Rect) -> Option<Action> {
        let painter = ui.painter().with_clip_rect(rect);
        let fg = theme().fg;
        // One faint tone for everything at rest: the logo, the labels and the keys.
        let dim = fg.gamma_multiply(0.22);
        let label_font = FontId::proportional(13.0);
        let cap_font = medium(11.5);

        let list = shortcuts();
        // The caps of every row are laid out first, so labels and caps line up in a column.
        let caps: Vec<Vec<(String, f32)>> = list
            .iter()
            .map(|item| {
                item.keys
                    .split('+')
                    .map(|part| {
                        let text = key_cap(part).to_string();
                        let galley =
                            painter.layout_no_wrap(text.clone(), cap_font.clone(), Color32::WHITE);
                        (text, (galley.size().x + CAP_PAD * 2.0).max(CAP_H))
                    })
                    .collect()
            })
            .collect();
        let keys_w = |row: &Vec<(String, f32)>| -> f32 {
            row.iter().map(|(_, w)| w).sum::<f32>() + CAP_GAP * (row.len() as f32 - 1.0)
        };
        let label_w = list
            .iter()
            .map(|item| {
                painter
                    .layout_no_wrap(item.label.into(), label_font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max);
        let caps_w = caps.iter().map(keys_w).fold(0.0_f32, f32::max);
        // 56pt of air between the two columns, like the reference, but never wider than the pane.
        let pad_x = 14.0;
        let block_w = (label_w + 56.0 + caps_w + pad_x * 2.0).min(rect.width() - 48.0);

        // The logo is barely there: the shortcuts are what the eye should land on.
        let logo_size = 104.0;
        let list_h = list.len() as f32 * ROW_H;
        let block_h = logo_size + 40.0 + list_h;
        let top = rect.center().y - block_h / 2.0;

        let logo = Rect::from_center_size(
            Pos2::new(rect.center().x, top + logo_size / 2.0),
            vec2(logo_size, logo_size),
        );
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(self.logo.id(), logo, uv, dim);

        let mut action = None;
        let mut y = logo.max.y + 40.0;
        for (item, row_caps) in list.iter().zip(&caps) {
            let row = Rect::from_center_size(
                Pos2::new(rect.center().x, y + ROW_H / 2.0),
                vec2(block_w, ROW_H - 4.0),
            );
            let id = ui.scope_id().with(("home", item.label));
            let response = ui.interact(row, id, Sense::click());
            if response.hovered() {
                painter.rect_filled(row, 8.0, theme().hover());
            }
            if response.clicked() {
                action = Some(item.action.clone());
            }
            painter.text(
                Pos2::new(row.min.x + pad_x, row.center().y),
                Align2::LEFT_CENTER,
                item.label,
                label_font.clone(),
                if response.hovered() { fg } else { dim },
            );
            // Caps from the right edge inwards, so the last key sits at the end of the row.
            let mut x = row.max.x - pad_x;
            for (text, width) in row_caps.iter().rev() {
                let cap = Rect::from_min_max(
                    Pos2::new(x - width, row.center().y - CAP_H / 2.0),
                    Pos2::new(x, row.center().y + CAP_H / 2.0),
                );
                painter.rect_filled(cap, 5.0, theme().hover());
                painter.rect_stroke(
                    cap,
                    5.0,
                    Stroke::new(1.0, theme().line()),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    cap.center(),
                    Align2::CENTER_CENTER,
                    text,
                    cap_font.clone(),
                    if response.hovered() {
                        theme().secondary()
                    } else {
                        dim
                    },
                );
                x -= width + CAP_GAP;
            }
            y += ROW_H;
        }
        action
    }
}
