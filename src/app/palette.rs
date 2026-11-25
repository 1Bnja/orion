//! Paleta de comandos (Ctrl+Shift+P; Cmd+Shift+P en Mac): buscar cualquier acción de Orion por su nombre,
//! Cada entrada muestra su atajo: así se aprenden sin memorizarlos.

use super::{Action, Orion};
use crate::colors::theme;
use eframe::egui;

#[derive(Default)]
pub(super) struct Palette {
    pub(super) open: bool,
    query: String,
    /// Fila resaltada (entre las que coinciden): Enter la ejecuta.
    selected: usize,
}

impl Palette {
    pub(super) fn toggle(&mut self) {
        *self = Self {
            open: !self.open,
            ..Self::default()
        };
    }
}

/// Minúsculas y sin tildes: "configuracion" encuentra "Configuración".
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' | 'ü' => 'u',
            c => c,
        })
        .collect()
}

/// Qué tan bien `name` coincide con `query` (menor = mejor), o None si no coincide.
/// 0: empieza así; 1: lo contiene; 2: tiene sus letras en orden ("ddere" → "Dividir a la derecha").
fn score(name: &str, query: &str) -> Option<u8> {
    let (name, query) = (fold(name), fold(query.trim()));
    if name.starts_with(&query) {
        return Some(0);
    }
    if name.contains(&query) {
        return Some(1);
    }
    let mut letters = name.chars();
    let mut wanted = query.chars().filter(|c| !c.is_whitespace());
    wanted.all(|q| letters.any(|c| c == q)).then_some(2)
}

/// Los índices de `names` que coinciden con `query`, de mejor a peor.
/// Si empatan, quedan en el orden de la lista.
fn ranked(names: &[&str], query: &str) -> Vec<usize> {
    let mut hits: Vec<(u8, usize)> = names
        .iter()
        .enumerate()
        .filter_map(|(i, name)| Some((score(name, query)?, i)))
        .collect();
    hits.sort();
    hits.into_iter().map(|(_, i)| i).collect()
}

impl Orion {
    /// Todo lo que se puede hacer desde la paleta: (nombre, atajo, acción).
    fn commands(&self) -> Vec<(String, &'static str, Action)> {
        let (_n, a) = (self.tabs.len(), self.active);
        let mut list: Vec<(String, &'static str, Action)> = vec![
            (
                "Nueva pestaña".into(),
                keys!("+Shift+T"),
                Action::NewTab(None),
            ),
            (
                "Nuevo proyecto (repo + agente)…".into(),
                keys!("+Shift+O"),
                Action::NewProject,
            ),
            ("Cerrar pestaña".into(), "", Action::CloseTab(a)),
            (
                "Siguiente pestaña".into(),
                "Ctrl+Tab",
                Action::SelectTab(self.tab_step(1)),
            ),
            (
                "Pestaña anterior".into(),
                "Ctrl+Shift+Tab",
                Action::SelectTab(self.tab_step(-1)),
            ),
            (
                "Dividir a la derecha".into(),
                keys!("+Shift+D"),
                Action::Split(true),
            ),
            (
                "Dividir hacia abajo".into(),
                keys!("+Shift+E"),
                Action::Split(false),
            ),
            ("Cerrar panel".into(), keys!("+Shift+W"), Action::ClosePane),
            ("Barra lateral".into(), keys!("+B"), Action::ToggleSidebar),
            ("Configuración".into(), keys!("+,"), Action::OpenSettings),
            ("Agrandar letra".into(), "", Action::FontSize(1.0)),
            ("Achicar letra".into(), "", Action::FontSize(-1.0)),
            ("Mostrar rendimiento".into(), "", Action::TogglePerf),
            (
                format!(
                    "Buscar actualizaciones (tienes la {})",
                    crate::update::CURRENT
                ),
                "",
                Action::CheckUpdates,
            ),
        ];
        // Alt+número en Windows, Cmd+número en Mac (ver `shortcuts`).
        const ALT: [&str; 9] = [
            "Alt+1", "Alt+2", "Alt+3", "Alt+4", "Alt+5", "Alt+6", "Alt+7", "Alt+8", "Alt+9",
        ];
        const CMD: [&str; 9] = [
            "Cmd+1", "Cmd+2", "Cmd+3", "Cmd+4", "Cmd+5", "Cmd+6", "Cmd+7", "Cmd+8", "Cmd+9",
        ];
        let numbers = if super::MAC { CMD } else { ALT };
        for &i in crate::pty::installed_shells() {
            let name = format!("Nueva pestaña: {}", crate::pty::SHELLS[i].name);
            list.push((name, "", Action::NewTab(Some(i))));
        }
        list.push((
            "Orquestar tarea con agentes…".into(),
            "",
            Action::Orchestrate,
        ));
        for i in self.providers() {
            let name = format!("Agente: {}", crate::agents::PROVIDERS[i].name);
            list.push((name, "", Action::Provider(i)));
        }
        for (i, agent) in self.settings.agents.iter().enumerate() {
            list.push((format!("Agente: {}", agent.name), "", Action::Agent(i)));
        }
        // Proyecto de la pestaña activa: una terminal u otro agente, sin tocar el mouse.
        if let Some(repo) = self.tabs[a].repo.as_deref() {
            let project = super::folder_name(repo);
            list.push((format!("Terminal en {project}"), "", Action::TerminalIn(a)));
            for choice in self.agent_choices() {
                let name = choice.name(&self.settings);
                let new_branch = format!("{name} en {project} (rama nueva)");
                list.push((new_branch, "", Action::InProject(a, choice, true)));
                let in_folder = format!("{name} en {project} (carpeta del repo)");
                list.push((in_folder, "", Action::InProject(a, choice, false)));
            }
        }
        list.push((
            "Limpiar worktrees de Orion".into(),
            "",
            Action::CleanWorktrees,
        ));
        // Proyectos guardados que no están abiertos.
        for (i, saved) in self.saved.iter().enumerate() {
            if !self.repo_busy(&saved.dir) {
                let name = format!(
                    "Abrir proyecto: {} · {}",
                    super::folder_name(&saved.dir),
                    saved.agent
                );
                list.push((name, "", Action::OpenSaved(i)));
            }
        }
        for (i, (name, _)) in crate::backdrop::BACKDROPS.iter().enumerate() {
            list.push((format!("Fondo: {name}"), "", Action::Backdrop(i)));
        }
        for (i, t) in crate::colors::THEMES.iter().enumerate() {
            list.push((format!("Tema: {}", t.name), "", Action::Theme(i)));
        }
        for (pos, i) in self.tab_order().into_iter().enumerate() {
            let name = format!("Ir a pestaña: {}", self.tabs[i].focused().title());
            let key = numbers.get(pos).copied().unwrap_or("");
            list.push((name, key, Action::SelectTab(i)));
        }
        list
    }

    /// Dibuja la paleta si está abierta. Devuelve la acción elegida (Enter o clic).
    pub(super) fn palette_window(&mut self, ctx: &egui::Context) -> Option<Action> {
        if !self.palette.open {
            return None;
        }
        let commands = self.commands();
        let names: Vec<&str> = commands.iter().map(|c| c.0.as_str()).collect();
        let shown = ranked(&names, &self.palette.query);
        let p = &mut self.palette;

        // Flechas, Enter y Escape son de la lista: se consumen antes de que el cuadro de texto las vea.
        let none = egui::Modifiers::NONE;
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(none, egui::Key::ArrowUp),
                i.consume_key(none, egui::Key::ArrowDown),
                i.consume_key(none, egui::Key::Enter),
                i.consume_key(none, egui::Key::Escape),
            )
        });
        let count = shown.len();
        if count > 0 {
            if down {
                p.selected = (p.selected + 1) % count;
            }
            if up {
                p.selected = (p.selected + count - 1) % count;
            }
        }
        p.selected = p.selected.min(count.saturating_sub(1));
        let mut chosen = (enter && count > 0).then_some(p.selected);

        let window = egui::Window::new("Paleta de comandos")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 48.0))
            .min_width(440.0)
            .max_width(440.0)
            .show(ctx, |ui| {
                let edit = egui::TextEdit::singleline(&mut p.query)
                    .hint_text("Escribe un comando…")
                    .desired_width(f32::INFINITY);
                let edit = ui.add(edit);
                if edit.changed() {
                    p.selected = 0;
                }
                edit.request_focus();
                ui.separator();
                if shown.is_empty() {
                    ui.weak("Nada coincide.");
                }
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for (row, &i) in shown.iter().enumerate() {
                            let (name, keys, _) = &commands[i];
                            let size = egui::vec2(ui.available_width(), 26.0);
                            let (rect, response) =
                                ui.allocate_exact_size(size, egui::Sense::click());
                            let painter = ui.painter();
                            if row == p.selected {
                                painter.rect_filled(rect, 4.0, theme().line());
                                if up || down {
                                    response.scroll_to_me(None);
                                }
                            } else if response.hovered() {
                                painter.rect_filled(rect, 4.0, theme().hover());
                            }
                            let left = rect.left_center() + egui::vec2(8.0, 0.0);
                            let font = egui::FontId::proportional(14.0);
                            painter.text(left, egui::Align2::LEFT_CENTER, name, font, theme().fg);
                            let right = rect.right_center() - egui::vec2(8.0, 0.0);
                            let small = egui::FontId::proportional(12.0);
                            painter.text(
                                right,
                                egui::Align2::RIGHT_CENTER,
                                keys,
                                small,
                                theme().ansi[8],
                            );
                            if response.clicked() {
                                chosen = Some(row);
                            }
                        }
                    });
            });
        // Escape o un clic fuera de la paleta la cierran.
        let clicked_outside = window.is_some_and(|w| w.response.clicked_elsewhere());
        if escape || clicked_outside || chosen.is_some() {
            p.open = false;
        }
        let index = shown.get(chosen?).copied()?;
        commands.into_iter().nth(index).map(|(_, _, action)| action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_search_ranks_matches() {
        let names = [
            "Nueva pestaña",
            "Dividir a la derecha",
            "Cerrar panel",
            "Configuración",
        ];
        assert_eq!(ranked(&names, ""), [0, 1, 2, 3]); // sin texto: todas, en orden
        assert_eq!(ranked(&names, "configuracion"), [3]); // sin tilde también
        assert_eq!(ranked(&names, "ddere"), [1]); // letras en orden
        // "pa": "Cerrar panel" lo contiene, "Nueva pestaña" solo tiene las letras en orden.
        assert_eq!(ranked(&names, "pa"), [2, 0]);
        assert!(ranked(&names, "xyz").is_empty());
    }
}
