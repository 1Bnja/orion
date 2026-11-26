//! La ventana de "Nuevo proyecto": un repo y uno o varios agentes, cada uno en su pestaña.
//! Con worktree, el agente trabaja en un git worktree y una rama nueva, como los harness de agentes.
//! Usa la misma base que la paleta y el orquestador (`overlay`).

use super::chrome::{Icon, draw_icon_size, draw_logo, medium};
use super::overlay::{self, Button, Kind, dim_text, faint_text, mix, panel_fill, tint};
use super::{AgentChoice, Orion, folder_name};
use crate::colors::theme;
use crate::logo::Logo;
use eframe::egui::{
    self, Color32, CursorIcon, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
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
    /// La rama actual de ese repo, para mostrarla.
    branch: Option<String>,
    /// Repos junto a `dir`.
    siblings: Vec<PathBuf>,
    pub(super) error: Option<String>,
}

impl Project {
    pub(super) fn set_dir(&mut self, dir: PathBuf) {
        let repo = crate::agents::repo_root(&dir);
        self.branch =
            (repo.as_ref().ok()).and_then(|root| crate::agents::current_branch(root).ok());
        self.repo = Some(repo);
        self.siblings = sibling_repos(&dir);
        self.path = dir.display().to_string();
        self.dir = Some(dir);
        self.error = None;
    }
}

const WIDTH: f32 = 660.0;
const PAD: i8 = 20;

/// Lo que se pidió desde la ventana en este frame: se aplica después de dibujarla.
#[derive(Default)]
struct Asked {
    close: bool,
    launch: bool,
    browse: bool,
    chosen: Option<PathBuf>,
    /// Marcar o desmarcar ese agente.
    toggle: Option<String>,
    worktree: Option<bool>,
}

/// El título de una sección, con una aclaración al lado.
fn section(ui: &mut Ui, title: &str, hint: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(medium(12.5)).color(theme().fg));
        ui.label(RichText::new(hint).size(11.5).color(faint_text()));
    });
}

fn note(ui: &mut Ui, text: &str, color: Color32) {
    ui.add(egui::Label::new(RichText::new(text).size(11.5).color(color)).wrap());
}

/// Una fila de carpetas para elegir con un clic; la elegida queda marcada.
fn folder_chips(
    ui: &mut Ui,
    label: &str,
    dirs: &[PathBuf],
    current: Option<&PathBuf>,
    asked: &mut Asked,
) {
    if dirs.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(label).size(11.5).color(faint_text()));
        for dir in dirs {
            let chip = overlay::chip(ui, &folder_name(dir), current == Some(dir), true, false);
            if chip.on_hover_text(dir.display().to_string()).clicked() {
                asked.chosen = Some(dir.clone());
            }
        }
    });
}

/// Una tarjeta de agente que se marca con un clic.
fn agent_card(
    ui: &mut Ui,
    name: &str,
    about: &str,
    on: bool,
    width: f32,
    logo: Logo,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 54.0), Sense::click());
    let ctx = ui.ctx().clone();
    let hover = ctx.animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.06);
    let sel = ctx.animate_bool_with_time(response.id.with("on"), on, 0.08);
    let body = if response.is_pointer_button_down_on() {
        rect.shrink(1.0)
    } else {
        rect
    };
    ui.painter().rect(
        body,
        10.0,
        tint(0.018 + 0.045 * sel + 0.025 * hover),
        Stroke::new(1.0, tint(0.10 + 0.14 * sel + 0.10 * hover)),
        StrokeKind::Inside,
    );
    let check =
        Rect::from_center_size(pos2(body.max.x - 19.0, body.min.y + 19.0), vec2(16.0, 16.0));
    if on {
        ui.painter().rect_filled(check, 5.0, theme().fg);
        draw_icon_size(ui, Icon::Check, check.center(), panel_fill(), 13.0);
    } else {
        let stroke = Stroke::new(1.0, tint(0.28));
        ui.painter()
            .rect_stroke(check, 5.0, stroke, StrokeKind::Inside);
    }
    draw_logo(
        ui,
        logo,
        pos2(body.min.x + 14.0, body.min.y + 19.0),
        theme().fg,
    );
    let line = |y: f32, right: f32| {
        Rect::from_min_max(
            pos2(body.min.x + 40.0, y - 9.0),
            pos2(body.max.x - right, y + 9.0),
        )
    };
    let color = if on { theme().fg } else { dim_text() };
    let painter = ui.painter();
    overlay::line_at(
        painter,
        line(body.min.y + 19.0, 34.0),
        name,
        medium(13.0),
        color,
    );
    let small = FontId::proportional(11.5);
    overlay::line_at(
        painter,
        line(body.min.y + 37.0, 10.0),
        about,
        small,
        faint_text(),
    );
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(about)
}

/// Un interruptor con título y descripción, como fila completa.
fn switch_row(ui: &mut Ui, title: &str, detail: &str, on: bool, enabled: bool) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), sense);
    let ctx = ui.ctx().clone();
    let hovered = enabled && response.hovered();
    let hover = ctx.animate_bool_with_time(response.id.with("hover"), hovered, 0.06);
    let t = ctx.animate_bool_with_time(response.id.with("on"), on, 0.1);
    ui.painter().rect(
        rect,
        10.0,
        tint(0.018 + 0.025 * hover),
        Stroke::new(1.0, tint(0.10 + 0.10 * hover)),
        StrokeKind::Inside,
    );
    // El interruptor: una pastilla con un círculo que se desliza.
    let track = Rect::from_center_size(pos2(rect.max.x - 34.0, rect.center().y), vec2(36.0, 20.0));
    let fade = if enabled { 1.0 } else { 0.5 };
    let track_color = mix(tint(0.16), theme().fg, t).gamma_multiply(fade);
    ui.painter().rect_filled(track, 10.0, track_color);
    let knob = pos2(track.min.x + 10.0 + 16.0 * t, track.center().y);
    let knob_color = mix(theme().fg, panel_fill(), t);
    ui.painter().circle_filled(knob, 7.0, knob_color);
    let line = |y: f32| {
        Rect::from_min_max(
            pos2(rect.min.x + 14.0, y - 9.0),
            pos2(track.min.x - 12.0, y + 9.0),
        )
    };
    let title_color = if enabled { theme().fg } else { dim_text() };
    let painter = ui.painter();
    overlay::line_at(
        painter,
        line(rect.min.y + 19.0),
        title,
        medium(13.0),
        title_color,
    );
    let small = FontId::proportional(11.5);
    overlay::line_at(
        painter,
        line(rect.min.y + 38.0),
        detail,
        small,
        faint_text(),
    );
    response.on_hover_cursor(if enabled {
        CursorIcon::PointingHand
    } else {
        CursorIcon::NotAllowed
    })
}

/// El campo de la ruta, con su ícono y un borde que se aclara con el foco.
fn path_field(ui: &mut Ui, p: &mut Project, width: f32, asked: &mut Asked) {
    let id = egui::Id::unique("proyecto-ruta");
    let focused = ui.memory(|m| m.has_focus(id));
    let focus = ui
        .ctx()
        .animate_bool_with_time(id.with("foco"), focused, 0.08);
    egui::Frame::new()
        .fill(tint(0.028))
        .stroke(Stroke::new(1.0, tint(0.14 + 0.22 * focus)))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(width - 22.0);
            // De izquierda a derecha aunque la fila venga de derecha a izquierda (ver `project_window`).
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let (icon, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                draw_icon_size(ui, Icon::Folder, icon.center(), dim_text(), 15.0);
                let hint = RichText::new("Pega la ruta del repo o usa Examinar")
                    .size(13.5)
                    .color(faint_text());
                let field = ui.add(
                    egui::TextEdit::singleline(&mut p.path)
                        .id(id)
                        .hint_text(hint)
                        .font(FontId::proportional(13.5))
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY),
                );
                if field.changed() {
                    p.error = None;
                }
                // Enter o salir del campo: usar esa ruta si cambió. Vacío no es un error.
                let typed = p.path.trim().to_string();
                let current = p.dir.as_ref().map(|d| d.display().to_string());
                if field.lost_focus() && !typed.is_empty() && current.as_deref() != Some(&typed) {
                    let typed = PathBuf::from(typed);
                    if typed.is_dir() {
                        asked.chosen = Some(typed);
                    } else {
                        p.error = Some("Esa carpeta no existe.".into());
                    }
                }
            });
        });
}

/// Lo que se sabe de la carpeta elegida, debajo del campo.
fn repo_status(ui: &mut Ui, p: &Project) {
    match (&p.error, &p.repo) {
        (Some(e), _) => note(ui, e, theme().ansi[1]),
        (None, Some(Ok(root))) => {
            let branch = (p.branch.as_deref()).map_or(String::new(), |b| format!(" · rama {b}"));
            ui.horizontal(|ui| {
                let (icon, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                draw_icon_size(ui, Icon::Check, icon.center(), dim_text(), 13.0);
                let text = format!("Repo git {}{branch}", folder_name(root));
                note(ui, &text, dim_text());
            });
        }
        (None, Some(Err(_))) => note(
            ui,
            "No es un repo git: los agentes pueden trabajar ahí, pero sin rama nueva.",
            theme().ansi[1],
        ),
        (None, None) => {}
    }
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

    /// Para qué sirve un agente, en una línea (lo que sale debajo de su nombre).
    fn about(&self, choice: AgentChoice) -> String {
        match choice {
            AgentChoice::Provider(i) => crate::agents::PROVIDERS[i].about.into(),
            AgentChoice::Profile(i) => format!("Perfil · {}", self.settings.agents[i].command),
        }
    }

    /// Por qué todavía no se puede abrir; None = se puede.
    fn project_blocker(&self) -> Option<&'static str> {
        let p = &self.project;
        if p.dir.is_none() {
            Some("Elige un repositorio")
        } else if p.picked.is_empty() {
            Some("Marca al menos un agente")
        } else if p.worktree && !p.repo.as_ref().is_some_and(|r| r.is_ok()) {
            Some("Para una rama nueva, la carpeta tiene que ser un repo git")
        } else {
            None
        }
    }

    /// Dibuja la ventana (abierta, o terminando de cerrarse). Devuelve lo que hay que abrir cuando
    /// aprietas "Abrir": la carpeta, los agentes marcados y si van en worktree.
    pub(super) fn project_window(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<(PathBuf, Vec<AgentChoice>, bool)> {
        let open = self.project.open;
        if !overlay::visible(ctx, "proyecto", open) {
            return None;
        }
        let choices: Vec<(AgentChoice, String, String)> = self
            .agent_choices()
            .into_iter()
            .map(|c| (c, c.name(&self.settings).to_string(), self.about(c)))
            .collect();
        let recents: Vec<PathBuf> = self.saved.iter().map(|s| s.dir.clone()).take(6).collect();
        let blocker = self.project_blocker();
        let mut asked = Asked::default();
        if open && !egui::Popup::is_any_open(ctx) {
            ctx.input_mut(|i| {
                asked.close = i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                asked.launch = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter);
            });
        }

        let logos: std::collections::HashMap<_, _> = choices
            .iter()
            .map(|(choice, name, _)| (name.clone(), choice.logo(&self.settings)))
            .collect();
        let p = &mut self.project;
        let shown = overlay::show(ctx, "proyecto", open, WIDTH, 0.07, |ui| {
            header(ui, &mut asked);
            overlay::divider(ui);
            egui::Frame::new().inner_margin(PAD).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);

                section(ui, "Repositorio", "la carpeta donde trabajarán");
                // El botón primero, desde la derecha: el campo ocupa justo lo que queda. La fila tiene
                // alto fijo: sin él, centrar en vertical estira el panel hasta el fondo de la ventana.
                let row = vec2(ui.available_width(), 36.0);
                let layout = egui::Layout::right_to_left(egui::Align::Center);
                ui.allocate_ui_with_layout(row, layout, |ui| {
                    if Button::new("Examinar…", Kind::Secondary).show(ui).clicked() {
                        asked.browse = true;
                    }
                    let width = ui.available_width();
                    path_field(ui, p, width, &mut asked);
                });
                repo_status(ui, p);
                folder_chips(ui, "Recientes", &recents, p.dir.as_ref(), &mut asked);
                folder_chips(ui, "Junto a este repo", &p.siblings, p.dir.as_ref(), &mut asked);

                section(ui, "Agentes", "marca uno o varios; cada uno abre su pestaña");
                if choices.is_empty() {
                    note(
                        ui,
                        "No encuentro agentes instalados (Claude Code, Codex, Gemini…) ni perfiles \
                         [[agent]] en config.toml.",
                        dim_text(),
                    );
                }
                let gap = 8.0;
                let width = (ui.available_width() - gap) / 2.0;
                for pair in choices.chunks(2) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for (_, name, about) in pair {
                            let on = p.picked.contains(name);
                            if agent_card(ui, name, about, on, width, logos[name]).clicked() {
                                asked.toggle = Some(name.clone());
                            }
                        }
                    });
                }

                section(ui, "Rama", "");
                let several = p.picked.len() > 1;
                let detail = if several {
                    "Con varios agentes, cada uno trabaja en su propia rama orion/… para no pisarse"
                } else {
                    "Crea un git worktree en .orion/worktrees y una rama orion/…; tu carpeta no se toca"
                };
                let title = "Trabajar en una rama nueva";
                if switch_row(ui, title, detail, p.worktree, !several).clicked() {
                    asked.worktree = Some(!p.worktree);
                }
            });
            overlay::divider(ui);
            footer(ui, p.picked.len(), blocker, &mut asked);
        })?;

        if !open {
            return None;
        }
        if shown.clicked_outside {
            asked.close = true;
        }
        self.apply_asked(asked, &choices)
    }

    /// Aplica lo pedido en la ventana. Devuelve lo que hay que abrir, si se apretó "Abrir".
    fn apply_asked(
        &mut self,
        mut asked: Asked,
        choices: &[(AgentChoice, String, String)],
    ) -> Option<(PathBuf, Vec<AgentChoice>, bool)> {
        if asked.close {
            self.project.open = false;
            return None;
        }
        if asked.browse {
            let start = self.project.dir.clone().unwrap_or_default();
            let dialog = rfd::FileDialog::new();
            let dialog = if start.is_dir() {
                dialog.set_directory(&start)
            } else {
                dialog
            };
            asked.chosen = dialog.pick_folder().or(asked.chosen);
        }
        if let Some(dir) = asked.chosen {
            self.project.set_dir(dir);
            self.refresh_worktree();
        }
        if let Some(name) = asked.toggle {
            let picked = &mut self.project.picked;
            if picked.contains(&name) {
                picked.retain(|n| n != &name);
            } else {
                picked.push(name);
            }
            self.refresh_worktree();
        }
        if let Some(on) = asked.worktree {
            self.project.worktree = on;
        }
        if !asked.launch || self.project_blocker().is_some() {
            return None;
        }
        let dir = self.project.dir.clone()?;
        let picks: Vec<AgentChoice> = (choices.iter())
            .filter(|(_, name, _)| self.project.picked.contains(name))
            .map(|(c, ..)| *c)
            .collect();
        // La ventana la cierra `launch_project` solo si salió bien: si falla, sigue abierta para
        // corregir y muestra el error. Cerrarla acá convertiría un fallo en algo invisible.
        if picks.is_empty() {
            self.project.error = Some("Los agentes marcados ya no están disponibles.".into());
            return None;
        }
        Some((dir, picks, self.project.worktree))
    }
}

fn header(ui: &mut Ui, asked: &mut Asked) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 62.0), Sense::hover());
    let tile = Rect::from_center_size(pos2(rect.min.x + 36.0, rect.center().y), vec2(34.0, 34.0));
    let stroke = Stroke::new(1.0, tint(0.16));
    ui.painter()
        .rect(tile, 9.0, tint(0.08), stroke, StrokeKind::Inside);
    draw_icon_size(ui, Icon::Folder, tile.center(), theme().fg, 18.0);
    let text = |y: f32| {
        Rect::from_min_max(
            pos2(tile.max.x + 12.0, y - 9.0),
            pos2(rect.max.x - 60.0, y + 9.0),
        )
    };
    let painter = ui.painter();
    let title = text(rect.center().y - 9.0);
    overlay::line_at(painter, title, "Nuevo proyecto", medium(15.5), theme().fg);
    overlay::line_at(
        painter,
        text(rect.center().y + 10.0),
        "Un repo y los agentes que trabajarán en él, cada uno en su pestaña",
        FontId::proportional(12.0),
        dim_text(),
    );
    let close = Rect::from_center_size(pos2(rect.max.x - 28.0, rect.center().y), vec2(28.0, 28.0));
    let mut corner = ui.new_child(egui::UiBuilder::new().max_rect(close));
    if overlay::icon_button(&mut corner, Icon::Close, "Cerrar (Esc)").clicked() {
        asked.close = true;
    }
}

fn footer(ui: &mut Ui, picked: usize, blocker: Option<&str>, asked: &mut Asked) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(PAD, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_height(34.0);
                overlay::keys(ui, "Esc");
                note(ui, "Cerrar", faint_text());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let text = match picked {
                        0 | 1 => "Abrir proyecto".to_string(),
                        n => format!("Abrir {n} agentes"),
                    };
                    let button = Button::new(&text, Kind::Primary)
                        .keys(keys!("+Enter"))
                        .enabled(blocker.is_none())
                        .show(ui);
                    if button.clicked() {
                        asked.launch = true;
                    }
                    if let Some(why) = blocker {
                        note(ui, why, faint_text());
                    }
                });
            });
        });
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
