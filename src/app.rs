//! La app: las pestañas, la configuración y lo que pasa en cada frame.
//!
//! Cada pestaña es una `Session` (su shell, su grilla, sus bloques). `session`, `render` y `menu`
//! son submódulos: al ser hijos de `app`, pueden usar los campos privados de `Orion`.

mod menu;
mod render;
mod session;

use crate::colors::{BG, FG, PALETTE};
use crate::config::Settings;
use crate::fonts::{apply_font, is_font_file};
use crate::pty::Error;
use alacritty_terminal::term::TermMode;
use eframe::egui;
use menu::SettingsMenu;
use session::Session;
use std::path::PathBuf;
use std::time::Instant;

/// Fondo de la barra de pestañas: un poco más oscuro que la terminal.
const TAB_BAR: egui::Color32 = egui::Color32::from_rgb(0x16, 0x16, 0x1e);

pub(crate) struct Orion {
    /// Las pestañas abiertas, de izquierda a derecha.
    sessions: Vec<Session>,
    /// La pestaña que se ve.
    active: usize,
    settings: Settings,
    menu: SettingsMenu,
    /// Si la ventana tenía el foco en el frame anterior (para avisar cambios a la app de la pestaña).
    focused: bool,
    /// Aviso breve ("Salida copiada") y desde cuándo se muestra.
    toast: Option<(String, Instant)>,
    /// Último título que le pusimos a la ventana (para cambiarlo solo cuando hace falta).
    window_title: String,
}

/// Algo que se pidió sobre las pestañas en este frame.
enum TabAction {
    New,
    Close(usize),
    Select(usize),
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
            sessions: vec![first],
            active: 0,
            settings,
            menu: SettingsMenu::default(),
            focused: true,
            toast: None,
            window_title: String::new(),
        })
    }

    fn session(&mut self) -> &mut Session {
        &mut self.sessions[self.active]
    }

    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// Abre una pestaña nueva del mismo tamaño que la actual, y la deja activa.
    fn new_tab(&mut self, ctx: &egui::Context) {
        let grid = self.session().grid;
        match Session::new(ctx, grid) {
            Ok(session) => {
                self.sessions.push(session);
                self.active = self.sessions.len() - 1;
            }
            Err(e) => self.show_toast(format!("No pude abrir otra pestaña: {e}")),
        }
    }

    /// Cierra una pestaña (y su shell). Si era la última, se cierra la ventana.
    fn close_tab(&mut self, ctx: &egui::Context, index: usize) {
        let mut session = self.sessions.remove(index);
        let _ = session.shell.child.kill();
        if self.sessions.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.active = active_after_close(self.active, index, self.sessions.len());
    }

    /// Barra de arriba: una pestaña por shell, la activa resaltada, × para cerrar y + para abrir otra.
    fn tab_bar(&self, ui: &mut egui::Ui) -> Option<TabAction> {
        let mut action = None;
        let frame = egui::Frame::new()
            .fill(TAB_BAR)
            .inner_margin(egui::Margin::symmetric(8, 5));
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                for (i, session) in self.sessions.iter().enumerate() {
                    let active = i == self.active;
                    let color = if active { FG } else { PALETTE[8] };
                    let title = egui::RichText::new(short_title(&session.title())).color(color);
                    let tab = egui::Button::new(title)
                        .fill(if active {
                            BG
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .corner_radius(6.0);
                    let tab = ui.add(tab).on_hover_text("Clic del medio para cerrar");
                    if tab.clicked() {
                        action = Some(TabAction::Select(i));
                    }
                    if tab.middle_clicked() {
                        action = Some(TabAction::Close(i));
                    }
                    let close =
                        egui::Button::new(egui::RichText::new("×").color(PALETTE[8])).frame(false);
                    if ui
                        .add(close)
                        .on_hover_text("Cerrar  (Ctrl+Shift+W)")
                        .clicked()
                    {
                        action = Some(TabAction::Close(i));
                    }
                    ui.add_space(6.0);
                }
                let plus =
                    egui::Button::new(egui::RichText::new("+").color(PALETTE[7])).frame(false);
                if ui
                    .add(plus)
                    .on_hover_text("Nueva pestaña  (Ctrl+Shift+T)")
                    .clicked()
                {
                    action = Some(TabAction::New);
                }
            });
        });
        action
    }

    /// Atajos de pestañas. Se "consumen": así no llegan a la shell.
    fn tab_shortcuts(&self, ui: &mut egui::Ui) -> Option<TabAction> {
        use egui::{Key, Modifiers};
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let count = self.sessions.len();
        ui.input_mut(|i| {
            if i.consume_key(ctrl_shift, Key::T) {
                return Some(TabAction::New);
            }
            if i.consume_key(ctrl_shift, Key::W) {
                return Some(TabAction::Close(self.active));
            }
            // Primero Ctrl+Shift+Tab: si no, Ctrl+Tab también lo atraparía.
            if i.consume_key(ctrl_shift, Key::Tab) {
                return Some(TabAction::Select((self.active + count - 1) % count));
            }
            if i.consume_key(Modifiers::CTRL, Key::Tab) {
                return Some(TabAction::Select((self.active + 1) % count));
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
            (n < count).then_some(TabAction::Select(n))
        })
    }

    fn apply_tab_action(&mut self, ctx: &egui::Context, action: TabAction) {
        match action {
            TabAction::New => self.new_tab(ctx),
            TabAction::Close(i) => self.close_tab(ctx, i),
            TabAction::Select(i) => self.active = i,
        }
    }
}

/// Qué pestaña queda activa al cerrar la `closed`, si quedan `left`.
/// La activa sigue siendo la misma pestaña: si cerraste una de su izquierda, su índice baja uno.
/// Si cerraste la activa, queda la de su derecha (o la de su izquierda si era la última).
fn active_after_close(active: usize, closed: usize, left: usize) -> usize {
    let active = if closed < active { active - 1 } else { active };
    active.min(left.saturating_sub(1))
}

/// Títulos cortos para que entren varias pestañas.
fn short_title(title: &str) -> String {
    const MAX: usize = 24;
    if title.chars().count() <= MAX {
        return title.to_string();
    }
    let cut: String = title.chars().take(MAX - 1).collect();
    format!("{cut}…")
}

impl eframe::App for Orion {
    // logic corre antes de cada ui y también con la ventana oculta: así ningún canal se acumula.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Todas las pestañas, no solo la visible: las de atrás siguen recibiendo su salida.
        for session in &mut self.sessions {
            session.process_output();
        }
        // Pestañas cuya shell terminó (escribiste `exit`): se cierran solas.
        while let Some(i) = self.sessions.iter_mut().position(|s| s.exited()) {
            self.close_tab(ctx, i);
            if self.sessions.is_empty() {
                return;
            }
        }
        // El título de la ventana sigue al de la pestaña activa.
        let title = format!("{} — Orion", self.session().title());
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.sessions.is_empty() {
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

        // Pestañas: barra de arriba y atajos.
        let shortcut = if self.menu.open {
            None
        } else {
            self.tab_shortcuts(ui)
        };
        if let Some(action) = self.tab_bar(ui).or(shortcut) {
            self.apply_tab_action(&ctx, action);
            if self.sessions.is_empty() {
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
        } else if !self.menu.open && !toggle {
            self.session().send_input(ui);
        }
        // Con el menú abierto, el teclado es para el menú (ej. el buscador), no para la shell.

        let area = ui.available_rect_before_wrap();
        let menu_open = self.menu.open;
        let open_menu = self.sessions[self.active].draw(ui, &self.settings.font, menu_open);
        if open_menu {
            self.open_menu();
        }
        if let Some(text) = self.session().toast.take() {
            self.show_toast(text);
        }
        if let Some(toast) = &self.toast {
            render::draw_toast(ui, area, toast, self.settings.font.size);
        }
        self.settings_window(&ctx);
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

    #[test]
    fn long_titles_get_shortened() {
        assert_eq!(short_title("Desktop"), "Desktop");
        let long = "una carpeta con un nombre larguísimo";
        assert_eq!(short_title(long).chars().count(), 24);
        assert!(short_title(long).ends_with('…'));
    }
}
