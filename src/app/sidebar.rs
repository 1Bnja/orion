//! The tab sidebar: the "new tab" button, one row per tab (grouped by repo), the saved projects
//! and the version at the bottom. Its right edge can be dragged to make it wider or narrower.

use super::chrome::{Icon, RADIUS, draw_icon, icon_button, label, medium};
use super::session::AgentStatus;
use super::{Action, MAC, Orion, Tab, default_shell, folder_name, render, tab_label};
use crate::blocks::Status;
use crate::colors::{surface, theme};
use crate::pty::{SHELLS, installed_shells};
use crate::update::{self, State as UpdateState};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, pos2, vec2};
use std::path::{Path, PathBuf};

/// A tab row has two lines: the name and a detail.
const TAB_ROW: f32 = 42.0;
/// The first line of a tab row is centered this far from its top.
const TITLE_Y: f32 = 13.0;
const DETAIL_Y: f32 = 29.0;
/// Rows with a single line (saved projects, group names, the version).
const ROW: f32 = 26.0;
/// Where the text of a row starts, after its dot.
const TEXT_X: f32 = 26.0;

/// The second line of a tab row: how its agent is doing or, without one, the last command
/// (the name of the shell if nothing ran yet).
fn tab_detail(tab: &Tab) -> String {
    let session = tab.focused();
    match tab.agent_status() {
        Some(AgentStatus::Running) => "En ejecución".into(),
        Some(AgentStatus::Done(Some(code))) if code != 0 => format!("Falló (código {code})"),
        Some(AgentStatus::Done(_)) => "Terminó".into(),
        // `Starting` says little: shells without blocks (cmd, WSL) never leave it.
        Some(AgentStatus::Starting) | None => session
            .last_command()
            .unwrap_or(SHELLS[session.kind].name)
            .to_string(),
    }
}

/// The dot of a tab: a ring while a command runs, a full dot when it ended (the color says
/// how) and a small quiet dot if nothing ran yet.
fn draw_status(painter: &egui::Painter, center: Pos2, activity: Option<Status>) {
    match activity {
        None => {
            painter.circle_filled(center, 2.0, theme().muted());
        }
        Some(status @ Status::Running(_)) => {
            let stroke = Stroke::new(1.5, render::status_color(status));
            painter.circle_stroke(center, 3.25, stroke);
        }
        Some(status) => {
            painter.circle_filled(center, 3.5, render::status_color(status));
        }
    }
}

/// The key that jumps to the tab in that place of the sidebar (only the first nine have one).
fn tab_shortcut(place: usize) -> Option<String> {
    let prefix = if MAC { "⌘" } else { "Alt+" };
    (place < 9).then(|| format!("{prefix}{}", place + 1))
}

/// Where the small button of a row goes: at its right, centered `line_y` below its top.
fn side_button(row: Rect, line_y: f32) -> Rect {
    Rect::from_center_size(pos2(row.max.x - 15.0, row.min.y + line_y), vec2(20.0, 20.0))
}

/// The name of a group of tabs. Repo groups have a "+" on the right (returned): it opens
/// another agent in that repo.
fn group_header(ui: &mut egui::Ui, name: &str, repo: Option<&Path>) -> Option<egui::Response> {
    ui.add_space(6.0);
    let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    let plus_rect = side_button(rect, ROW / 2.0);
    let text = Rect::from_min_max(
        rect.min + vec2(10.0, 0.0),
        pos2(plus_rect.min.x - 4.0, rect.max.y),
    );
    label(ui.painter(), text, name, medium(11.5), theme().muted());
    let repo = repo?;
    let plus = icon_button(ui, plus_rect, row.id.with("mas"), Icon::Plus);
    row.on_hover_text(repo.display().to_string());
    Some(plus.on_hover_text("Otro agente en este proyecto"))
}

impl Orion {
    pub(super) fn tab_sidebar(&self, ui: &mut egui::Ui) -> Option<Action> {
        // Hidden: the panel is not drawn and the terminal uses the full width.
        if self.sidebar_hidden {
            return None;
        }
        let mut action = None;
        let margin = egui::Margin {
            left: 8,
            right: 8,
            top: 2,
            bottom: 6,
        };
        let frame = egui::Frame::new()
            .fill(surface(theme().sidebar()))
            .inner_margin(margin);
        egui::Panel::left("pestañas")
            .resizable(true)
            .default_size(208.0)
            .min_size(140.0)
            .max_size(360.0)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                self.new_tab_button(ui, &mut action);
                ui.add_space(8.0);

                // The version stays at the bottom; the rows scroll in what is left above it.
                let rest = ui.available_rect_before_wrap();
                let (list, footer) = rest.split_top_bottom_at_y(rest.max.y - ROW);
                let mut rows = ui.new_child(egui::UiBuilder::new().max_rect(list));
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .show(&mut rows, |ui| {
                        self.tab_rows(ui, &mut action);
                        self.saved_rows(ui, &mut action);
                    });
                self.footer(ui, footer, &mut action);
            });
        action
    }

    /// "Nueva pestaña": a click opens a terminal. The arrow (or a right click) opens the menu
    /// with projects, other shells and agents.
    fn new_tab_button(&self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        let (rect, new) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        let more_rect = rect.with_min_x(rect.max.x - 28.0);
        let more = ui.interact(more_rect, new.id.with("menu"), Sense::click());

        let painter = ui.painter();
        let fill = if new.hovered() || more.hovered() {
            theme().hover()
        } else {
            Color32::TRANSPARENT
        };
        let line = Stroke::new(1.0, theme().frame_line());
        painter.rect(rect, RADIUS, fill, line, egui::StrokeKind::Inside);
        painter.vline(more_rect.min.x, rect.y_range().shrink(8.0), line);
        let color = |hovered: bool| {
            if hovered {
                theme().fg
            } else {
                theme().secondary()
            }
        };
        let plus = pos2(rect.min.x + 14.0, rect.center().y);
        draw_icon(ui, Icon::Plus, plus, color(new.hovered()));
        let text = Rect::from_min_max(
            rect.min + vec2(TEXT_X, 0.0),
            pos2(more_rect.min.x - 4.0, rect.max.y),
        );
        let font = FontId::proportional(13.0);
        label(painter, text, "Nueva pestaña", font, color(new.hovered()));
        draw_icon(ui, Icon::Chevron, more_rect.center(), color(more.hovered()));

        if new.clicked() {
            *action = Some(Action::NewTab(None));
        }
        let open = if new.secondary_clicked() {
            Some(egui::SetOpenCommand::Bool(true))
        } else if more.clicked() {
            Some(egui::SetOpenCommand::Toggle)
        } else {
            None
        };
        let providers = self.providers();
        let broken = self.broken_providers();
        egui::Popup::menu(&new).open_memory(open).show(|ui| {
            ui.weak("Proyecto");
            if ui
                .button("Nuevo proyecto (repo + agente)…")
                .on_hover_text("Elige un repo y un agente; con worktree, cada uno en su rama.")
                .clicked()
            {
                *action = Some(Action::NewProject);
                ui.close();
            }
            ui.separator();
            ui.weak("Terminal");
            let default = default_shell(&self.settings);
            if ui
                .button(format!("Terminal ({})", SHELLS[default].name))
                .clicked()
            {
                *action = Some(Action::NewTab(None));
                ui.close();
            }
            for &i in installed_shells() {
                if i != default && ui.button(format!("Terminal: {}", SHELLS[i].name)).clicked() {
                    *action = Some(Action::NewTab(Some(i)));
                    ui.close();
                }
            }
            if !providers.is_empty() || !broken.is_empty() || !self.settings.agents.is_empty() {
                ui.separator();
                ui.weak("Agente en esta carpeta");
            }
            for &i in &providers {
                let provider = &crate::agents::PROVIDERS[i];
                let button = ui.button(format!("Agente: {}", provider.name));
                if button.on_hover_text(provider.about).clicked() {
                    *action = Some(Action::Provider(i));
                    ui.close();
                }
            }
            for (i, missing) in &broken {
                let provider = &crate::agents::PROVIDERS[*i];
                ui.add_enabled(
                    false,
                    egui::Button::new(format!("Agente: {} (roto)", provider.name)),
                )
                .on_disabled_hover_text(format!(
                    "Instalación rota: falta {}. Reinstálalo ({})",
                    missing.display(),
                    provider.about
                ));
            }
            for (i, agent) in self.settings.agents.iter().enumerate() {
                if ui.button(format!("Agente: {}", agent.name)).clicked() {
                    *action = Some(Action::Agent(i));
                    ui.close();
                }
            }
        });
        let tip = keys!(
            "",
            "+Shift+T",
            "\n▾ o clic derecho: proyecto, shell o agente"
        );
        new.on_hover_text(tip);
    }

    /// One row per tab, grouped by repo: the repos in the order they appear and the loose
    /// terminals last. Without any repo open the list is flat.
    fn tab_rows(&self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        let has_repos = self.tabs.iter().any(|t| t.repo.is_some());
        let mut shown: Option<Option<&PathBuf>> = None;
        for (place, i) in self.tab_order().into_iter().enumerate() {
            let tab = &self.tabs[i];
            if has_repos && shown != Some(tab.repo.as_ref()) {
                let name = tab.repo.as_deref().map_or("Terminales".into(), folder_name);
                if let Some(plus) = group_header(ui, &name, tab.repo.as_deref()) {
                    self.repo_menu(&plus, i, &name, action);
                }
                shown = Some(tab.repo.as_ref());
            }
            self.tab_row(ui, i, place, action);
        }
    }

    /// The menu of a repo group: another agent in that repo (in a new branch or in its
    /// folder), or a terminal. `tab` is any tab of the repo.
    fn repo_menu(
        &self,
        plus: &egui::Response,
        tab: usize,
        name: &str,
        action: &mut Option<Action>,
    ) {
        let choices = self.agent_choices();
        egui::Popup::menu(plus).show(|ui| {
            for (worktree, title) in [
                (true, format!("Nueva rama en {name}")),
                (false, "En la carpeta del repo".to_string()),
            ] {
                ui.weak(title);
                for &choice in &choices {
                    if ui.button(choice.name(&self.settings)).clicked() {
                        *action = Some(Action::InProject(tab, choice, worktree));
                        ui.close();
                    }
                }
                if choices.is_empty() {
                    ui.weak("No hay agentes");
                }
                ui.separator();
            }
            if ui.button("Terminal en la carpeta del repo").clicked() {
                *action = Some(Action::TerminalIn(tab));
                ui.close();
            }
        });
    }

    /// The row of tab `i`, which is in that `place` of the sidebar.
    fn tab_row(&self, ui: &mut egui::Ui, i: usize, place: usize, action: &mut Option<Action>) {
        let tab = &self.tabs[i];
        // With splits, the row shows the focused pane.
        let session = tab.focused();
        let size = vec2(ui.available_width(), TAB_ROW);
        let (rect, row) = ui.allocate_exact_size(size, Sense::click());
        let close_rect = side_button(rect, TITLE_Y);
        let hovered = row.hovered() || ui.rect_contains_pointer(close_rect);
        let active = i == self.active;
        let painter = ui.painter();
        if active {
            // The active tab has the color of the terminal: it reads as part of it.
            let line = Stroke::new(1.0, theme().line());
            painter.rect(rect, RADIUS, theme().bg, line, egui::StrokeKind::Inside);
        } else if hovered {
            painter.rect_filled(rect, RADIUS, theme().hover());
        }

        // A pane of the tab asking for attention replaces the dot with a pulse of the accent.
        let dot = rect.min + vec2(14.0, TITLE_Y);
        let alert = tab
            .panes
            .leaves()
            .into_iter()
            .find_map(|s| s.attention.clone());
        if alert.is_some() {
            let time = ui.input(|i| i.time) as f32;
            let pulse = (time * 4.0).sin() * 0.5 + 0.5;
            let ring = theme().accent().gamma_multiply(0.35 * (1.0 - pulse));
            painter.circle_filled(dot, 4.0 + 4.0 * pulse, ring);
            painter.circle_filled(dot, 4.0, theme().accent());
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        } else {
            draw_status(painter, dot, session.activity());
        }

        // On the right of the first line: the shortcut or, under the mouse, the close button.
        let mut title_end = rect.max.x - 10.0;
        if hovered {
            title_end = close_rect.min.x - 4.0;
        } else if let Some(shortcut) = tab_shortcut(place) {
            let font = FontId::proportional(11.0);
            let pos = pos2(rect.max.x - 10.0, rect.min.y + TITLE_Y);
            let drawn = painter.text(
                pos,
                egui::Align2::RIGHT_CENTER,
                shortcut,
                font,
                theme().muted(),
            );
            title_end = drawn.min.x - 8.0;
        }

        let line = |y: f32, end: f32| {
            Rect::from_x_y_ranges(
                rect.min.x + TEXT_X..=end,
                rect.min.y + y - 9.0..=rect.min.y + y + 9.0,
            )
        };
        let title = tab_label(tab.agent.as_deref(), &session.title());
        let (font, color) = if active {
            (medium(13.0), theme().fg)
        } else {
            (FontId::proportional(13.0), theme().secondary())
        };
        label(painter, line(TITLE_Y, title_end), title, font, color);
        let detail = line(DETAIL_Y, rect.max.x - 10.0);
        let font = FontId::proportional(11.5);
        label(painter, detail, tab_detail(tab), font, theme().muted());

        if hovered {
            let close = icon_button(ui, close_rect, row.id.with("cerrar"), Icon::Close);
            if close.on_hover_text("Cerrar pestaña").clicked() {
                *action = Some(Action::CloseTab(i));
            }
        }
        let mut tip = match session.last_command() {
            Some(command) => format!("Último comando: {command}\nClic del medio para cerrar"),
            None => "Clic del medio para cerrar".into(),
        };
        if let Some(alert) = &alert {
            tip = format!("🔔 {alert}\n{tip}");
        }
        let row = row.on_hover_text(tip);
        // Right click on a tab with a worktree: see, merge or discard the agent's work.
        if let (Some(_), Some(repo)) = (&tab.worktree, &tab.repo) {
            row.context_menu(|ui| {
                if ui.button("Ver cambios").clicked() {
                    *action = Some(Action::Review(i));
                    ui.close();
                }
                // ponytail: git on the window thread, only while the menu is open.
                let into = crate::agents::current_branch(repo).unwrap_or_else(|_| "?".into());
                if ui.button(format!("Fusionar en {into}")).clicked() {
                    *action = Some(Action::Merge(i));
                    ui.close();
                }
                if ui.button("Descartar rama…").clicked() {
                    *action = Some(Action::Review(i));
                    ui.close();
                }
            });
        }
        if row.clicked() {
            *action = Some(Action::SelectTab(i));
        }
        if row.middle_clicked() {
            *action = Some(Action::CloseTab(i));
        }
    }

    /// Saved projects without an open tab: a click opens them again.
    fn saved_rows(&self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        let closed: Vec<usize> = (0..self.saved.len())
            .filter(|&i| !self.repo_busy(&self.saved[i].dir))
            .collect();
        if !closed.is_empty() {
            group_header(ui, "Guardados", None);
        }
        for i in closed {
            let saved = &self.saved[i];
            let size = vec2(ui.available_width(), ROW);
            let (rect, row) = ui.allocate_exact_size(size, Sense::click());
            let close_rect = side_button(rect, ROW / 2.0);
            let hovered = row.hovered() || ui.rect_contains_pointer(close_rect);
            if hovered {
                ui.painter().rect_filled(rect, RADIUS, theme().hover());
            }
            // The folder and, after it and quieter, its agent.
            let mut text = Rect::from_min_max(
                rect.min + vec2(10.0, 0.0),
                pos2(close_rect.min.x - 4.0, rect.max.y),
            );
            let font = FontId::proportional(13.0);
            let name = folder_name(&saved.dir);
            text.min.x += label(ui.painter(), text, name, font, theme().secondary()) + 8.0;
            let font = FontId::proportional(11.5);
            label(ui.painter(), text, &saved.agent, font, theme().muted());

            if hovered {
                let close = icon_button(ui, close_rect, row.id.with("olvidar"), Icon::Close);
                if close.on_hover_text("Quitar de los guardados").clicked() {
                    *action = Some(Action::ForgetSaved(i));
                }
            }
            if row.on_hover_text(saved.dir.display().to_string()).clicked() {
                *action = Some(Action::OpenSaved(i));
            }
        }
    }

    /// The last line of the sidebar: Orion's version (a click looks for a new one) or, when
    /// there is an update, what to do with it.
    fn footer(&self, ui: &egui::Ui, rect: Rect, action: &mut Option<Action>) {
        let update = theme().ansi[2];
        let (text, color, next, tip) = match self.updater.state() {
            UpdateState::Available(version) => (
                format!("↑  Actualizar a {version}"),
                update,
                Some(Action::InstallUpdate(version)),
                "Descarga la versión nueva, la verifica y la deja lista",
            ),
            UpdateState::Installing(version) => (
                format!("Descargando {version}…"),
                update,
                None,
                "Puedes seguir usando Orion mientras tanto",
            ),
            UpdateState::Ready => (
                "↻  Reiniciar Orion".into(),
                update,
                Some(Action::RestartForUpdate),
                "Se cierran las pestañas y Orion abre la versión nueva",
            ),
            _ => (
                format!("Orion {}", update::CURRENT),
                theme().muted(),
                Some(Action::CheckUpdates),
                "Buscar actualizaciones",
            ),
        };
        let row = ui.interact(rect, ui.scope_id().with("version"), Sense::click());
        if row.hovered() && next.is_some() {
            ui.painter().rect_filled(rect, RADIUS, theme().hover());
        }
        let text_rect = rect.shrink2(vec2(10.0, 0.0));
        label(
            ui.painter(),
            text_rect,
            text,
            FontId::proportional(11.5),
            color,
        );
        if row.on_hover_text(tip).clicked() {
            *action = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_nine_tabs_have_a_shortcut() {
        let first = tab_shortcut(0).unwrap();
        assert!(first.ends_with('1'), "{first}");
        assert!(tab_shortcut(8).unwrap().ends_with('9'));
        assert_eq!(tab_shortcut(9), None);
    }
}
