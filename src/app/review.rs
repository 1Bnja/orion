//! La ventana "Ver cambios" de una pestaña con worktree: qué hizo el agente en su rama,
//! y desde ahí fusionarla o descartarla.

use super::{Action, Orion};
use crate::agents::{self, BranchSummary};
use eframe::egui;
use std::path::PathBuf;

pub(super) struct Review {
    pub(super) worktree: PathBuf,
    pub(super) branch: String,
    /// La rama del repo principal, donde se fusiona.
    pub(super) into: String,
    pub(super) summary: Result<BranchSummary, String>,
    /// Ya apretó "Descartar rama…": falta que confirme.
    confirm: bool,
}

impl Review {
    /// Lee el estado de git de la pestaña. ponytail: en el hilo de la ventana (git tarda poco).
    pub(super) fn load(repo: &std::path::Path, worktree: PathBuf) -> Result<Self, String> {
        let branch = agents::current_branch(&worktree)?;
        Ok(Self {
            summary: agents::branch_summary(repo, &worktree, &branch),
            into: agents::current_branch(repo).unwrap_or_else(|_| "?".into()),
            worktree,
            branch,
            confirm: false,
        })
    }
}

impl Orion {
    /// Dibuja la ventana si está abierta. Devuelve la acción que se pidió desde ella.
    pub(super) fn review_window(&mut self, ctx: &egui::Context) -> Option<Action> {
        let review = self.review.as_mut()?;
        let tab = self
            .tabs
            .iter()
            .position(|t| t.worktree.as_deref() == Some(review.worktree.as_path()));
        let Some(tab) = tab else {
            self.review = None; // la pestaña ya no existe
            return None;
        };
        let (mut open, mut action) = (true, None);
        egui::Window::new(format!("Cambios de {}", review.branch))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 48.0))
            .min_width(480.0)
            .max_width(480.0)
            .show(ctx, |ui| {
                match &review.summary {
                    Err(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    Ok(s) => {
                        if s.commits.is_empty() {
                            ui.label("La rama no tiene commits propios.");
                        } else {
                            ui.strong(format!("{} commits en {}", s.commits.len(), review.branch));
                        }
                        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                            for line in &s.commits {
                                ui.monospace(line);
                            }
                            if !s.stat.is_empty() {
                                ui.add_space(6.0);
                                ui.monospace(&s.stat);
                            }
                        });
                        if s.dirty {
                            ui.add_space(4.0);
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                "Hay cambios sin commitear en el worktree.",
                            );
                        }
                    }
                }
                ui.add_space(8.0);
                if review.confirm {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!(
                            "Se borran el worktree y la rama {}, incluidos los cambios sin guardar. No se puede deshacer.",
                            review.branch
                        ),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Descartar").clicked() {
                            action = Some(Action::Discard(tab));
                        }
                        if ui.button("Cancelar").clicked() {
                            review.confirm = false;
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        if ui.button(format!("Fusionar en {}", review.into)).clicked() {
                            action = Some(Action::Merge(tab));
                        }
                        if ui.button("Descartar rama…").clicked() {
                            review.confirm = true;
                        }
                    });
                }
            });
        if !open || ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.review = None;
        }
        action
    }
}
