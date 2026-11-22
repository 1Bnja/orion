//! La ventana de "Nuevo proyecto": un repo y un agente, en su propia pestaña.
//! Con worktree, el agente trabaja en un git worktree y una rama nueva, como los harness de agentes.

use super::{AgentChoice, Orion};
use crate::colors::theme;
use eframe::egui;
use std::path::PathBuf;

/// Un proyecto guardado: sobrevive al cierre de Orion y se reabre con un clic desde la barra lateral.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub(super) struct Saved {
    /// Raíz del repo.
    pub(super) dir: PathBuf,
    /// Nombre del agente (de `PROVIDERS` o de un `[[agent]]`), tal como sale en el menú.
    pub(super) agent: String,
    pub(super) worktree: bool,
}

impl Saved {
    /// Agrega el proyecto o, si ese repo ya estaba, actualiza su agente y su worktree.
    pub(super) fn upsert(list: &mut Vec<Saved>, saved: Saved) {
        match list.iter_mut().find(|s| s.dir == saved.dir) {
            Some(old) => *old = saved,
            None => list.push(saved),
        }
    }
}

/// projects.json, al lado de config.toml.
fn saved_path() -> Option<PathBuf> {
    crate::config::config_path().map(|p| p.with_file_name("projects.json"))
}

pub(super) fn load_saved() -> Vec<Saved> {
    let text = saved_path().and_then(|p| std::fs::read_to_string(p).ok());
    text.and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub(super) fn save_saved(list: &[Saved]) {
    // ponytail: si no se puede escribir, el proyecto solo no sobrevive al cierre; no vale un aviso.
    let Some(path) = saved_path() else { return };
    if let Ok(text) = serde_json::to_string_pretty(list) {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
        let _ = std::fs::write(path, text);
    }
}

#[derive(Default)]
pub(super) struct Project {
    pub(super) open: bool,
    /// Carpeta elegida (el repo).
    pub(super) dir: Option<PathBuf>,
    /// Índice en `Orion::agent_choices()`.
    pub(super) agent: usize,
    pub(super) worktree: bool,
    /// Raíz del repo de `dir`, o el error de git si no es un repo.
    repo: Option<Result<PathBuf, String>>,
    pub(super) error: Option<String>,
}

impl Project {
    pub(super) fn set_dir(&mut self, dir: PathBuf) {
        self.repo = Some(crate::agents::repo_root(&dir));
        self.dir = Some(dir);
    }
}

impl Orion {
    /// Dibuja la ventana si está abierta. Devuelve el proyecto a abrir cuando aprietas "Abrir".
    pub(super) fn project_window(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<(PathBuf, AgentChoice, bool)> {
        if !self.project.open {
            return None;
        }
        let choices: Vec<(AgentChoice, String, bool)> = self
            .agent_choices()
            .into_iter()
            .map(|c| {
                (
                    c,
                    c.name(&self.settings).to_string(),
                    c.worktree(&self.settings),
                )
            })
            .collect();
        if self.project.agent >= choices.len() {
            self.project.agent = 0;
        }
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut open = !escape;
        let mut launch = false;
        let start = self.project.dir.clone().unwrap_or_default();
        egui::Window::new("Nuevo proyecto")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 48.0))
            .min_width(480.0)
            .max_width(480.0)
            .show(ctx, |ui| {
                ui.weak(
                    "Un repo y un agente, en su propia pestaña. Con worktree, \
                     cada agente trabaja en su rama.",
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Elegir repo…").clicked() {
                        let dialog = rfd::FileDialog::new();
                        let dialog = if start.is_dir() {
                            dialog.set_directory(&start)
                        } else {
                            dialog
                        };
                        if let Some(dir) = dialog.pick_folder() {
                            self.project.set_dir(dir);
                        }
                    }
                    match &self.project.dir {
                        Some(dir) => {
                            ui.label(egui::RichText::new(dir.display().to_string()).monospace());
                        }
                        None => {
                            ui.weak("Ninguna carpeta elegida");
                        }
                    }
                });
                if let Some(Err(e)) = &self.project.repo {
                    ui.colored_label(theme().ansi[1], format!("No es un repo git: {e}"));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Agente");
                    let current = choices
                        .get(self.project.agent)
                        .map(|c| c.1.as_str())
                        .unwrap_or("—");
                    egui::ComboBox::from_id_salt("agente-proyecto")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for (i, (_, name, worktree)) in choices.iter().enumerate() {
                                if ui
                                    .selectable_label(self.project.agent == i, name.as_str())
                                    .clicked()
                                {
                                    self.project.agent = i;
                                    self.project.worktree = *worktree;
                                }
                            }
                        });
                });
                ui.checkbox(
                    &mut self.project.worktree,
                    "Crear un git worktree y una rama nueva (orion/…)",
                );
                if self.project.worktree
                    && let Some(Ok(root)) = &self.project.repo
                {
                    ui.weak(format!("Rama nueva en {}", root.display()));
                }
                if let Some(e) = &self.project.error {
                    ui.colored_label(theme().ansi[1], e);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let repo_ok = !self.project.worktree
                        || self.project.repo.as_ref().is_some_and(|r| r.is_ok());
                    let ready = self.project.dir.is_some() && repo_ok && !choices.is_empty();
                    if ui
                        .add_enabled(ready, egui::Button::new("Abrir proyecto"))
                        .clicked()
                    {
                        launch = true;
                    }
                    if choices.is_empty() {
                        ui.weak("No hay agentes instalados ni perfiles en config.toml.");
                    }
                });
            });
        self.project.open = open;
        if !launch {
            return None;
        }
        let dir = self.project.dir.clone()?;
        let choice = choices.get(self.project.agent)?.0;
        Some((dir, choice, self.project.worktree))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(dir: &str, agent: &str) -> Saved {
        Saved {
            dir: dir.into(),
            agent: agent.into(),
            worktree: true,
        }
    }

    #[test]
    fn saving_the_same_repo_twice_keeps_one_entry() {
        let mut list = vec![saved("/a", "Claude Code")];
        Saved::upsert(&mut list, saved("/b", "Codex"));
        Saved::upsert(&mut list, saved("/a", "Aider"));
        assert_eq!(list, vec![saved("/a", "Aider"), saved("/b", "Codex")]);
    }

    #[test]
    fn saved_projects_round_trip_through_json() {
        let list = vec![saved("C:/Users/ana/mi repo", "Claude Code")];
        let text = serde_json::to_string(&list).unwrap();
        assert_eq!(serde_json::from_str::<Vec<Saved>>(&text).unwrap(), list);
    }
}
