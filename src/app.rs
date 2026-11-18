//! La app: las pestañas, la configuración y lo que pasa en cada frame.
//!
//! Cada pestaña tiene uno o más paneles (splits) y cada panel es una `Session` (su shell, su grilla,
//! sus bloques). `session`, `panes`, `render` y `menu` son submódulos: al ser hijos de `app`,
//! pueden usar los campos privados de `Orion`.

mod menu;
mod palette;
mod panes;
mod render;
mod session;

use crate::colors::{BG, FG, PALETTE};
use crate::config::Settings;
use crate::fonts::{apply_font, is_font_file};
use crate::pty::Error;
use alacritty_terminal::term::TermMode;
use eframe::egui;
use menu::SettingsMenu;
use palette::Palette;
use panes::Split;
use session::Session;
use std::path::PathBuf;
use std::time::Instant;

/// Fondo de la barra de pestañas: un poco más oscuro que la terminal.
const TAB_BAR: egui::Color32 = egui::Color32::from_rgb(0x16, 0x16, 0x1e);

/// Una pestaña: sus paneles y cuál tiene el foco.
struct Tab {
    panes: Split<Session>,
    /// El `id` del panel que recibe el teclado.
    focus: u64,
    /// Dónde se dibujó cada panel en el último frame (para mover el foco con Ctrl+Alt+flechas).
    rects: Vec<(u64, egui::Rect)>,
}

impl Tab {
    fn new(session: Session) -> Self {
        Self {
            focus: session.id,
            panes: Split::new(session),
            rects: Vec::new(),
        }
    }

    fn focused(&self) -> &Session {
        self.panes
            .get(self.focus)
            .expect("el foco apunta a un panel")
    }

    fn focused_mut(&mut self) -> &mut Session {
        self.panes
            .get_mut(self.focus)
            .expect("el foco apunta a un panel")
    }
}

pub(crate) struct Orion {
    /// Las pestañas abiertas, de arriba abajo.
    tabs: Vec<Tab>,
    /// La pestaña que se ve.
    active: usize,
    settings: Settings,
    menu: SettingsMenu,
    palette: Palette,
    /// Si la ventana tenía el foco en el frame anterior (para avisar cambios a la app de la pestaña).
    focused: bool,
    /// Aviso breve ("Salida copiada") y desde cuándo se muestra.
    toast: Option<(String, Instant)>,
    /// Último título que le pusimos a la ventana (para cambiarlo solo cuando hace falta).
    window_title: String,
}

/// Algo que se pidió sobre las pestañas o los paneles en este frame.
enum Action {
    NewTab,
    CloseTab(usize),
    SelectTab(usize),
    /// Dividir el panel con foco: a la derecha (true) o hacia abajo.
    Split(bool),
    ClosePane,
    /// Mover el foco al panel de al lado, ej. (1, 0) = el de la derecha.
    Focus(egui::Vec2),
    /// Abrir o cerrar la paleta de comandos.
    Palette,
    OpenSettings,
    /// Cambiar el tamaño de la letra en tantos puntos.
    FontSize(f32),
}

impl Orion {
    /// Prepara la app cuando egui ya creó la ventana: la fuente y la primera pestaña.
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        grid: (u16, u16),
    ) -> Result<Self, Error> {
        apply_font(&cc.egui_ctx, &settings.font.family);
        let first = Session::new(&cc.egui_ctx, grid)?;
        Ok(Self {
            tabs: vec![Tab::new(first)],
            active: 0,
            settings,
            menu: SettingsMenu::default(),
            palette: Palette::default(),
            focused: true,
            toast: None,
            window_title: String::new(),
        })
    }

    /// El panel con foco de la pestaña activa.
    fn session(&mut self) -> &mut Session {
        self.tabs[self.active].focused_mut()
    }

    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// Abre una pestaña nueva del mismo tamaño que la actual, y la deja activa.
    fn new_tab(&mut self, ctx: &egui::Context) {
        let grid = self.session().grid;
        match Session::new(ctx, grid) {
            Ok(session) => {
                self.tabs.push(Tab::new(session));
                self.active = self.tabs.len() - 1;
            }
            Err(e) => self.show_toast(format!("No pude abrir otra pestaña: {e}")),
        }
    }

    /// Cierra una pestaña (y las shells de todos sus paneles). Si era la última, se cierra la ventana.
    fn close_tab(&mut self, ctx: &egui::Context, index: usize) {
        let mut tab = self.tabs.remove(index);
        for session in tab.panes.leaves_mut() {
            let _ = session.shell.child.kill();
        }
        if self.tabs.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.active = active_after_close(self.active, index, self.tabs.len());
    }

    /// Divide el panel con foco con una shell nueva, que queda con el foco.
    fn split(&mut self, ctx: &egui::Context, right: bool) {
        let grid = self.session().grid; // el panel nuevo se ajusta a su tamaño al dibujarse
        match Session::new(ctx, grid) {
            Ok(session) => {
                let id = session.id;
                let tab = &mut self.tabs[self.active];
                if tab.panes.split(tab.focus, right, session).is_ok() {
                    tab.focus = id;
                }
            }
            Err(e) => self.show_toast(format!("No pude abrir otro panel: {e}")),
        }
    }

    /// Cierra el panel `id` de la pestaña `index` (y su shell). Si era el único, se cierra la pestaña.
    fn close_pane(&mut self, ctx: &egui::Context, index: usize, id: u64) {
        let tab = &mut self.tabs[index];
        if tab.panes.leaves().len() == 1 {
            self.close_tab(ctx, index);
            return;
        }
        // El foco pasa al vecino más cercano: el de la izquierda, arriba, derecha o abajo.
        let dirs = [(-1.0, 0.0), (0.0, -1.0), (1.0, 0.0), (0.0, 1.0)];
        let next = dirs
            .into_iter()
            .find_map(|(x, y)| panes::neighbor(&tab.rects, id, egui::vec2(x, y)));
        if let Some(mut session) = tab.panes.remove(id) {
            let _ = session.shell.child.kill();
        }
        if tab.panes.get(tab.focus).is_none() {
            let first = tab.panes.leaves()[0].id;
            tab.focus = next
                .filter(|&n| tab.panes.get(n).is_some())
                .unwrap_or(first);
        }
    }

    /// Barra lateral de pestañas (como las pestañas verticales de Warp, pero más simple):
    /// "+ Nueva pestaña" arriba y una fila por pestaña con un punto de estado y el nombre de la carpeta.
    /// Se puede ensanchar o angostar arrastrando su borde.
    fn tab_sidebar(&self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        let frame = egui::Frame::new()
            .fill(TAB_BAR)
            .inner_margin(egui::Margin::symmetric(6, 8));
        let panel = egui::Panel::left("pestañas")
            .resizable(true)
            .default_size(190.0)
            .min_size(120.0)
            .max_size(360.0)
            .show_separator_line(false)
            .frame(frame);
        panel.show(ui, |ui| {
            let width = ui.available_width();
            let font = egui::FontId::proportional(13.0);

            // Fila "+ Nueva pestaña".
            let (rect, new) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
            if new.hovered() {
                ui.painter().rect_filled(rect, 6.0, BG);
            }
            let color = if new.hovered() { FG } else { PALETTE[8] };
            let text_pos = rect.left_center() + egui::vec2(10.0, 0.0);
            ui.painter().text(
                text_pos,
                egui::Align2::LEFT_CENTER,
                "+  Nueva pestaña",
                font.clone(),
                color,
            );
            if new.on_hover_text("Ctrl+Shift+T").clicked() {
                action = Some(Action::NewTab);
            }
            ui.add_space(6.0);

            for (i, tab) in self.tabs.iter().enumerate() {
                // Con splits, la fila muestra el panel con foco.
                let session = tab.focused();
                let (rect, row) =
                    ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
                let active = i == self.active;
                let painter = ui.painter();
                if active {
                    // La activa: fondo como el de la terminal y una línea azul a la izquierda.
                    painter.rect_filled(rect, 6.0, BG);
                    let accent = egui::Rect::from_min_size(
                        rect.min + egui::vec2(0.0, 7.0),
                        egui::vec2(3.0, 16.0),
                    );
                    painter.rect_filled(accent, 1.5, PALETTE[4]);
                } else if row.hovered() {
                    painter.rect_filled(rect, 6.0, egui::Color32::from_rgb(0x1f, 0x20, 0x2c));
                }

                // Punto de estado: cómo terminó el último comando (o amarillo si algo corre).
                let dot = session.activity().map_or(PALETTE[8], render::status_color);
                painter.circle_filled(rect.left_center() + egui::vec2(14.0, 0.0), 3.5, dot);

                // × a la derecha, solo al pasar el mouse por la fila.
                let close_rect = egui::Rect::from_center_size(
                    rect.right_center() - egui::vec2(14.0, 0.0),
                    egui::vec2(20.0, 20.0),
                );
                let close = ui.interact(close_rect, row.id.with("cerrar"), egui::Sense::click());

                // Nombre de la carpeta, recortado para que no pise la ×.
                let color = if active { FG } else { PALETTE[7] };
                let title_rect = egui::Rect::from_min_max(
                    rect.min + egui::vec2(26.0, 0.0),
                    egui::pos2(close_rect.min.x - 4.0, rect.max.y),
                );
                let galley = ui
                    .painter()
                    .layout_no_wrap(session.title(), font.clone(), color);
                ui.painter().with_clip_rect(title_rect).galley(
                    title_rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0),
                    galley,
                    color,
                );

                if row.hovered() || close.hovered() {
                    let x_color = if close.hovered() { FG } else { PALETTE[8] };
                    ui.painter().text(
                        close_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "×",
                        font.clone(),
                        x_color,
                    );
                }
                if close.on_hover_text("Cerrar pestaña").clicked() {
                    action = Some(Action::CloseTab(i));
                }
                let tip = match session.last_command() {
                    Some(command) => {
                        format!("Último comando: {command}\nClic del medio para cerrar")
                    }
                    None => "Clic del medio para cerrar".into(),
                };
                let row = row.on_hover_text(tip);
                if row.clicked() {
                    action = Some(Action::SelectTab(i));
                }
                if row.middle_clicked() {
                    action = Some(Action::CloseTab(i));
                }
                ui.add_space(2.0);
            }
        });
        action
    }

    /// Atajos de pestañas y paneles. Se "consumen": así no llegan a la shell.
    fn shortcuts(&self, ui: &mut egui::Ui) -> Option<Action> {
        use egui::{Key, Modifiers};
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let count = self.tabs.len();
        ui.input_mut(|i| {
            if i.consume_key(ctrl_shift, Key::P) {
                return Some(Action::Palette);
            }
            if i.consume_key(ctrl_shift, Key::T) {
                return Some(Action::NewTab);
            }
            // Splits (como en Warp): D = a la derecha, E = hacia abajo. W cierra el panel.
            if i.consume_key(ctrl_shift, Key::D) {
                return Some(Action::Split(true));
            }
            if i.consume_key(ctrl_shift, Key::E) {
                return Some(Action::Split(false));
            }
            if i.consume_key(ctrl_shift, Key::W) {
                return Some(Action::ClosePane);
            }
            // Ctrl+Alt+flechas: pasar al panel de al lado.
            let arrows = [
                (Key::ArrowLeft, egui::vec2(-1.0, 0.0)),
                (Key::ArrowRight, egui::vec2(1.0, 0.0)),
                (Key::ArrowUp, egui::vec2(0.0, -1.0)),
                (Key::ArrowDown, egui::vec2(0.0, 1.0)),
            ];
            for (key, dir) in arrows {
                if i.consume_key(Modifiers::CTRL | Modifiers::ALT, key) {
                    return Some(Action::Focus(dir));
                }
            }
            // Primero Ctrl+Shift+Tab: si no, Ctrl+Tab también lo atraparía.
            if i.consume_key(ctrl_shift, Key::Tab) {
                return Some(Action::SelectTab((self.active + count - 1) % count));
            }
            if i.consume_key(Modifiers::CTRL, Key::Tab) {
                return Some(Action::SelectTab((self.active + 1) % count));
            }
            // Alt+1 … Alt+9: ir a esa pestaña.
            const NUMBERS: [Key; 9] = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            let n = NUMBERS
                .iter()
                .position(|&key| i.consume_key(Modifiers::ALT, key))?;
            (n < count).then_some(Action::SelectTab(n))
        })
    }

    fn apply_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::NewTab => self.new_tab(ctx),
            Action::CloseTab(i) => self.close_tab(ctx, i),
            Action::SelectTab(i) => self.active = i,
            Action::Split(right) => self.split(ctx, right),
            Action::ClosePane => {
                let id = self.tabs[self.active].focus;
                self.close_pane(ctx, self.active, id);
            }
            Action::Focus(dir) => {
                let tab = &mut self.tabs[self.active];
                if let Some(id) = panes::neighbor(&tab.rects, tab.focus, dir) {
                    tab.focus = id;
                }
            }
            Action::Palette => self.palette.toggle(),
            Action::OpenSettings => self.open_menu(),
            Action::FontSize(delta) => {
                let mut new = self.settings.clone();
                new.font.size = (new.font.size + delta).clamp(8.0, 48.0);
                self.change_settings(ctx, new);
            }
        }
    }

    /// Dibuja los paneles de la pestaña activa en `area`.
    /// Devuelve true si algún panel pidió abrir la configuración.
    fn draw_panes(&mut self, ui: &mut egui::Ui, area: egui::Rect) -> bool {
        let ctx = ui.ctx().clone();
        // Con el menú o la paleta abiertos, la rueda y el mouse son para ellos.
        let menu_open = self.menu.open || self.palette.open;
        let tab = &mut self.tabs[self.active];
        // Entre panel y panel queda una línea de 2 px del color de la barra lateral.
        ui.painter().rect_filled(area, 0.0, TAB_BAR);
        tab.rects.clear();
        tab.panes.layout(area, 2.0, &mut tab.rects);

        // Clic en un panel: pasa a tener el foco (si el clic no fue en un menú o ventana de encima).
        let press = ui.input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()));
        if let Some(pos) = press.flatten()
            && ctx.layer_id_at(pos) == Some(ui.layer_id())
            && let Some(&(id, _)) = tab.rects.iter().find(|(_, r)| r.contains(pos))
        {
            tab.focus = id;
        }

        let several = tab.rects.len() > 1;
        let (mut open_menu, mut split, mut toast) = (false, None, None);
        for &(id, rect) in &tab.rects {
            let Some(session) = tab.panes.get_mut(id) else {
                continue;
            };
            // Cada panel dibuja en su pedazo; id_salt separa los ids de egui de un panel y otro.
            let mut pane_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(id));
            let active = id == tab.focus;
            open_menu |= session.draw(&mut pane_ui, &self.settings.font, menu_open, active);
            if several && !active {
                // Los paneles sin foco, un poco más oscuros: se ve dónde vas a escribir.
                let dim = egui::Color32::from_black_alpha(60);
                ui.painter().rect_filled(rect, 0.0, dim);
            }
            if let Some(right) = session.split_request.take() {
                tab.focus = id;
                split = Some(right);
            }
            toast = session.toast.take().or(toast);
        }
        if let Some(right) = split {
            self.split(&ctx, right);
        }
        if let Some(text) = toast {
            self.show_toast(text);
        }
        open_menu
    }
}

/// Qué pestaña queda activa al cerrar la `closed`, si quedan `left`.
/// La activa sigue siendo la misma pestaña: si cerraste una de su izquierda, su índice baja uno.
/// Si cerraste la activa, queda la de su derecha (o la de su izquierda si era la última).
fn active_after_close(active: usize, closed: usize, left: usize) -> usize {
    let active = if closed < active { active - 1 } else { active };
    active.min(left.saturating_sub(1))
}

impl eframe::App for Orion {
    // logic corre antes de cada ui y también con la ventana oculta: así ningún canal se acumula.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Todos los paneles de todas las pestañas: los de atrás siguen recibiendo su salida.
        for tab in &mut self.tabs {
            for session in tab.panes.leaves_mut() {
                session.process_output();
            }
        }
        // Paneles cuya shell terminó (escribiste `exit`): se cierran solos.
        while let Some((t, id)) = self.tabs.iter_mut().enumerate().find_map(|(t, tab)| {
            let mut sessions = tab.panes.leaves_mut().into_iter();
            sessions.find_map(|s| s.exited().then_some((t, s.id)))
        }) {
            self.close_pane(ctx, t, id);
            if self.tabs.is_empty() {
                return;
            }
        }
        // El título de la ventana sigue al del panel con foco.
        let title = format!("{} — Orion", self.session().title());
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.tabs.is_empty() {
            return; // la ventana se está cerrando
        }
        let ctx = ui.ctx().clone();

        // Si la app lo pidió (ESC[?1004h, ej. vim), avisarle cuando la ventana gana o pierde el foco.
        let focused = ui.input(|i| i.focused);
        if focused != self.focused {
            self.focused = focused;
            let session = self.session();
            if session.term.mode().contains(TermMode::FOCUS_IN_OUT) {
                session.write(if focused { b"\x1b[I" } else { b"\x1b[O" });
            }
        }

        // Archivos de fuente arrastrados a la ventana.
        let dropped: Vec<PathBuf> = ui.input(|i| {
            let files = i.raw.dropped_files.iter();
            files.map(|f| f.path().to_path_buf()).collect()
        });
        for path in dropped {
            if is_font_file(&path.to_string_lossy()) {
                self.font_dropped(&ctx, &path);
            }
        }

        // Pestañas y paneles: barra lateral y atajos.
        let shortcut = if self.menu.open {
            None
        } else {
            self.shortcuts(ui)
        };
        if let Some(action) = self.tab_sidebar(ui).or(shortcut) {
            self.apply_action(&ctx, action);
            if self.tabs.is_empty() {
                return;
            }
        }

        // Ctrl+, abre o cierra la configuración (como en VS Code); Escape la cierra.
        let (toggle, escape) = ui.input(|i| {
            let toggle = i.modifiers.ctrl && i.key_pressed(egui::Key::Comma);
            (toggle, i.key_pressed(egui::Key::Escape))
        });
        if toggle && !self.menu.open {
            self.open_menu();
        } else if self.menu.open && (toggle || escape) {
            self.menu.open = false;
        } else if !self.menu.open && !self.palette.open && !toggle {
            self.session().send_input(ui);
        }
        // Con el menú o la paleta abiertos, el teclado es para ellos (sus buscadores), no para la shell.

        let area = ui.available_rect_before_wrap();
        if self.draw_panes(ui, area) {
            self.open_menu();
        }
        if let Some(toast) = &self.toast {
            render::draw_toast(ui, area, toast, self.settings.font.size);
        }
        self.settings_window(&ctx);
        // Al final: una acción de la paleta (ej. cerrar la última pestaña) puede cerrar la ventana.
        if let Some(action) = self.palette_window(&ctx) {
            self.apply_action(&ctx, action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_tabs_keeps_the_right_one_active() {
        // 4 pestañas, la activa es la 2.
        assert_eq!(active_after_close(2, 0, 3), 1); // cerré una de la izquierda: se corre
        assert_eq!(active_after_close(2, 3, 3), 2); // cerré una de la derecha: no cambia
        assert_eq!(active_after_close(2, 2, 3), 2); // cerré la activa: queda la de su derecha
        assert_eq!(active_after_close(3, 3, 3), 2); // cerré la activa y era la última: la de la izquierda
    }
}
