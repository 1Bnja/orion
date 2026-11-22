//! La ventana de "Nuevo proyecto": un repo y uno o varios agentes, cada uno en su pestaña.
//! Con worktree, el agente trabaja en un git worktree y una rama nueva, como los harness de agentes.

use super::{AgentChoice, Orion, folder_name};
use crate::colors::theme;
use eframe::egui;
use std::path::{Path, PathBuf};

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
    /// Pone el proyecto de primero (el más reciente); si ese repo ya estaba, reemplaza su entrada.
    pub(super) fn upsert(list: &mut Vec<Saved>, saved: Saved) {
        list.retain(|s| s.dir != saved.dir);
        list.insert(0, saved);
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

/// Los repos que están junto a `dir` (en la misma carpeta madre), para elegirlos con un clic.
/// Los worktrees no cuentan: su `.git` es un archivo, no una carpeta.
fn sibling_repos(dir: &Path) -> Vec<PathBuf> {
    let Some(entries) = dir.parent().and_then(|p| std::fs::read_dir(p).ok()) else {
        return Vec::new();
    };
    let mut repos: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p != dir && p.join(".git").is_dir())
        .collect();
    repos.sort();
    repos.truncate(12);
    repos
}

#[derive(Default)]
pub(super) struct Project {
    pub(super) open: bool,
    /// Carpeta elegida (el repo).
    pub(super) dir: Option<PathBuf>,
    /// Lo que dice el campo de la ruta (puede ir por delante de `dir` mientras escribes).
    path: String,
    /// Nombres de los agentes marcados. Con varios, cada uno va en su propia rama.
    pub(super) picked: Vec<String>,
    pub(super) worktree: bool,
    /// Raíz del repo de `dir`, o el error de git si no es un repo.
    repo: Option<Result<PathBuf, String>>,
    /// Repos junto a `dir`.
    siblings: Vec<PathBuf>,
    pub(super) error: Option<String>,
}

impl Project {
    pub(super) fn set_dir(&mut self, dir: PathBuf) {
        self.repo = Some(crate::agents::repo_root(&dir));
        self.siblings = sibling_repos(&dir);
        self.path = dir.display().to_string();
        self.dir = Some(dir);
        self.error = None;
    }
}

/// Una fila de carpetas para elegir con un clic.
fn chips(ui: &mut egui::Ui, label: &str, dirs: &[PathBuf], chosen: &mut Option<PathBuf>) {
    if dirs.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.weak(label);
    ui.horizontal_wrapped(|ui| {
        for dir in dirs {
            let chip = ui.small_button(folder_name(dir));
            if chip.on_hover_text(dir.display().to_string()).clicked() {
                *chosen = Some(dir.clone());
            }
        }
    });
}

impl Orion {
    /// Si `choice` debe ir en un worktree por defecto, según el repo elegido en la ventana.
    pub(super) fn default_worktree(&self, choice: AgentChoice) -> bool {
        let busy = matches!(&self.project.repo, Some(Ok(root)) if self.repo_busy(root));
        choice.worktree(&self.settings, busy)
    }

    /// Ajusta el worktree por defecto a lo marcado: con varios agentes, cada uno va en su rama;
    /// con uno, según si el repo ya está ocupado.
    pub(super) fn refresh_worktree(&mut self) {
        let picked: Vec<AgentChoice> = self
            .agent_choices()
            .into_iter()
            .filter(|c| {
                self.project
                    .picked
                    .iter()
                    .any(|n| n == c.name(&self.settings))
            })
            .collect();
        self.project.worktree =
            picked.len() > 1 || picked.first().is_some_and(|&c| self.default_worktree(c));
    }

    /// Dibuja la ventana si está abierta. Devuelve lo que hay que abrir cuando aprietas "Abrir":
    /// la carpeta, los agentes marcados y si van en worktree.
    pub(super) fn project_window(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<(PathBuf, Vec<AgentChoice>, bool)> {
        if !self.project.open {
            return None;
        }
        let choices: Vec<(AgentChoice, String)> = self
            .agent_choices()
            .into_iter()
            .map(|c| (c, c.name(&self.settings).to_string()))
            .collect();
        let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut open = !escape;
        let mut launch = false;
        let mut chosen: Option<PathBuf> = None;
        let start = self.project.dir.clone().unwrap_or_default();
        let recents: Vec<PathBuf> = self
            .saved
            .iter()
            .map(|s| s.dir.clone())
            .filter(|d| Some(d) != self.project.dir.as_ref())
            .take(6)
            .collect();
        egui::Window::new("Nuevo proyecto")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 48.0))
            .min_width(480.0)
            .max_width(480.0)
            .show(ctx, |ui| {
                ui.weak(
                    "Un repo y uno o varios agentes, cada uno en su pestaña. \
                     Con worktree, cada agente trabaja en su rama.",
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
                        chosen = dialog.pick_folder();
                    }
                    let field = egui::TextEdit::singleline(&mut self.project.path)
                        .hint_text("o pega la ruta del repo")
                        .desired_width(ui.available_width());
                    let field = ui.add(field);
                    // Enter o salir del campo: usar esa ruta si cambió.
                    let current = self.project.dir.as_ref().map(|d| d.display().to_string());
                    if field.lost_focus() && current.as_deref() != Some(self.project.path.trim()) {
                        let typed = PathBuf::from(self.project.path.trim());
                        if typed.is_dir() {
                            chosen = Some(typed);
                        } else {
                            self.project.error = Some("Esa carpeta no existe.".into());
                        }
                    }
                });
                chips(ui, "Recientes", &recents, &mut chosen);
                chips(
                    ui,
                    "Junto a este repo",
                    &self.project.siblings.clone(),
                    &mut chosen,
                );
                if let Some(Err(e)) = &self.project.repo {
                    ui.colored_label(theme().ansi[1], format!("No es un repo git: {e}"));
                }
                ui.add_space(6.0);
                ui.label("Agentes (marca uno o varios)");
                let mut toggled = false;
                ui.horizontal_wrapped(|ui| {
                    for (_, name) in &choices {
                        let on = self.project.picked.contains(name);
                        if ui.selectable_label(on, name.as_str()).clicked() {
                            if on {
                                self.project.picked.retain(|n| n != name);
                            } else {
                                self.project.picked.push(name.clone());
                            }
                            toggled = true;
                        }
                    }
                });
                if toggled {
                    self.refresh_worktree();
                }
                let several = self.project.picked.len() > 1;
                let text = if several {
                    "Cada agente en su propio worktree y rama nueva (orion/…)"
                } else {
                    "Crear un git worktree y una rama nueva (orion/…)"
                };
                ui.add_enabled(
                    !several,
                    egui::Checkbox::new(&mut self.project.worktree, text),
                );
                if self.project.worktree
                    && let Some(Ok(root)) = &self.project.repo
                {
                    ui.weak(format!("Ramas nuevas en {}", root.display()));
                }
                if let Some(e) = &self.project.error {
                    ui.colored_label(theme().ansi[1], e);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let repo_ok = !self.project.worktree
                        || self.project.repo.as_ref().is_some_and(|r| r.is_ok());
                    let ready =
                        self.project.dir.is_some() && repo_ok && !self.project.picked.is_empty();
                    let label = match self.project.picked.len() {
                        0 | 1 => "Abrir proyecto".to_string(),
                        n => format!("Abrir {n} agentes"),
                    };
                    if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
                        launch = true;
                    }
                    if choices.is_empty() {
                        ui.weak("No hay agentes instalados ni perfiles en config.toml.");
                    }
                });
            });
        self.project.open = open;
        if let Some(dir) = chosen {
            self.project.set_dir(dir);
            self.refresh_worktree();
        }
        if !launch {
            return None;
        }
        let dir = self.project.dir.clone()?;
        let picks: Vec<AgentChoice> = choices
            .iter()
            .filter(|(_, name)| self.project.picked.contains(name))
            .map(|(c, _)| *c)
            .collect();
        (!picks.is_empty()).then_some((dir, picks, self.project.worktree))
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
    fn saving_the_same_repo_twice_keeps_one_entry_and_puts_it_first() {
        let mut list = vec![saved("/a", "Claude Code")];
        Saved::upsert(&mut list, saved("/b", "Codex"));
        assert_eq!(list[0], saved("/b", "Codex"));
        Saved::upsert(&mut list, saved("/a", "Aider"));
        assert_eq!(list, vec![saved("/a", "Aider"), saved("/b", "Codex")]);
    }

    #[test]
    fn saved_projects_round_trip_through_json() {
        let list = vec![saved("C:/Users/ana/mi repo", "Claude Code")];
        let text = serde_json::to_string(&list).unwrap();
        assert_eq!(serde_json::from_str::<Vec<Saved>>(&text).unwrap(), list);
    }

    #[test]
    fn sibling_repos_skip_worktrees_and_plain_folders() {
        let base = std::env::temp_dir().join(format!("orion-sib-{}", std::process::id()));
        let (me, other) = (base.join("yo"), base.join("otro"));
        std::fs::create_dir_all(me.join(".git")).unwrap();
        std::fs::create_dir_all(other.join(".git")).unwrap();
        std::fs::create_dir_all(base.join("carpeta")).unwrap(); // sin .git
        std::fs::create_dir_all(base.join("wt")).unwrap();
        std::fs::write(base.join("wt/.git"), "gitdir: x").unwrap(); // un worktree
        assert_eq!(sibling_repos(&me), vec![other]);
        let _ = std::fs::remove_dir_all(&base);
    }
}
