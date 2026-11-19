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

use crate::colors::{opaque, surface, theme};
use crate::config::{Settings, config_modified, reload_settings};
use crate::fonts::{apply_font, is_font_file};
use crate::pty::{Error, SHELLS, find_shell, installed_shells, usable_shell};
use crate::update::{self, State as UpdateState, Updater};
use alacritty_terminal::term::TermMode;
use eframe::egui;
use menu::SettingsMenu;
use palette::Palette;
use panes::Split;
use session::Session;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

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
    /// Fecha de config.toml la última vez que lo miramos, y cuándo fue eso.
    config_modified: Option<SystemTime>,
    config_checked: Instant,
    /// Some = se muestra el medidor de rendimiento.
    perf: Option<Perf>,
    /// La ventana para Windows (HWND): para pedirle Mica o acrílico.
    hwnd: Option<isize>,
    updater: Updater,
    /// Buscaste actualizaciones a mano: avisar también si está al día o si falló.
    /// (La revisión automática al abrir es silenciosa: sin internet no hay nada que molestar.)
    update_asked: bool,
}

/// Medidor de rendimiento (paleta → "Mostrar rendimiento"): cuánto tarda cada frame y cuántos hay por segundo.
struct Perf {
    /// Tiempo de CPU por frame en ms, promediado para que el número no salte.
    avg_ms: f32,
    frames: u32,
    fps: u32,
    since: Instant,
}

impl Perf {
    fn new() -> Self {
        Self {
            avg_ms: 0.0,
            frames: 0,
            fps: 0,
            since: Instant::now(),
        }
    }

    /// Suma un frame que tardó `cpu` segundos (lo mide eframe: nuestro código + preparar los triángulos).
    fn frame(&mut self, cpu: f32) {
        self.avg_ms = self.avg_ms * 0.9 + cpu * 1000.0 * 0.1;
        self.frames += 1;
        if self.since.elapsed() >= Duration::from_secs(1) {
            (self.fps, self.frames, self.since) = (self.frames, 0, Instant::now());
        }
    }
}

/// Cada cuánto se revisa si config.toml cambió.
/// ponytail: se pregunta la fecha del archivo (stdlib); el crate `notify` avisaría al instante, sin preguntar.
const CONFIG_POLL: Duration = Duration::from_secs(1);

/// Algo que se pidió sobre las pestañas o los paneles en este frame.
enum Action {
    /// Pestaña nueva con esa shell (índice en `pty::SHELLS`), o con la de config.toml.
    NewTab(Option<usize>),
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
    /// Usar el tema con ese índice en `colors::THEMES`.
    Theme(usize),
    TogglePerf,
    /// Usar el fondo con ese índice en `backdrop::BACKDROPS`.
    Backdrop(usize),
    CheckUpdates,
    /// Bajar e instalar esa versión.
    InstallUpdate(String),
    RestartForUpdate,
    /// Abrir el agente con ese índice en `settings.agents`.
    Agent(usize),
}

impl Orion {
    /// Prepara la app cuando egui ya creó la ventana: la fuente y la primera pestaña.
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        grid: (u16, u16),
    ) -> Result<Self, Error> {
        apply_font(&cc.egui_ctx, &settings.font.family);
        menu::apply_theme(&cc.egui_ctx, &settings.theme);
        let first = Session::new(&cc.egui_ctx, grid, default_shell(&settings))?;
        let orion = Self {
            tabs: vec![Tab::new(first)],
            active: 0,
            settings,
            menu: SettingsMenu::default(),
            palette: Palette::default(),
            focused: true,
            toast: None,
            window_title: String::new(),
            config_modified: config_modified(),
            config_checked: Instant::now(),
            perf: None,
            hwnd: crate::backdrop::hwnd_of(cc),
            updater: Updater::new(),
            update_asked: false,
        };
        orion.apply_window_look();
        // Al abrir, revisar si hay una versión nueva. En debug (`cargo run`) no: sería la del .exe de desarrollo.
        if orion.settings.check_updates && !cfg!(debug_assertions) {
            orion.updater.check(&cc.egui_ctx);
        }
        Ok(orion)
    }

    /// El panel con foco de la pestaña activa.
    fn session(&mut self) -> &mut Session {
        self.tabs[self.active].focused_mut()
    }

    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// Abre una pestaña nueva del mismo tamaño que la actual, y la deja activa.
    /// `kind`: qué shell (índice en `pty::SHELLS`); None = la de config.toml.
    fn new_tab(&mut self, ctx: &egui::Context, kind: Option<usize>) {
        let grid = self.session().grid;
        let kind = kind.unwrap_or_else(|| default_shell(&self.settings));
        match Session::new(ctx, grid, kind) {
            Ok(session) => {
                self.tabs.push(Tab::new(session));
                self.active = self.tabs.len() - 1;
            }
            Err(e) => self.show_toast(format!("No pude abrir otra pestaña: {e}")),
        }
    }

    /// Pestaña nueva que lanza un agente: en la carpeta actual o, si el perfil lo pide, en un worktree nuevo.
    fn open_agent(&mut self, ctx: &egui::Context, index: usize) {
        let Some(agent) = self.settings.agents.get(index).cloned() else {
            return;
        };
        let mut dir = self.session().cwd.clone();
        if agent.worktree {
            let Some(cwd) = dir else {
                let tip = "abre el agente desde una pestaña de PowerShell dentro del repo";
                return self.show_toast(format!("No sé en qué repo estás: {tip}"));
            };
            match crate::agents::create_worktree(&cwd, &agent.name) {
                Ok(path) => dir = Some(path),
                Err(e) => return self.show_toast(format!("No pude crear el worktree: {e}")),
            }
        }
        let tabs = self.tabs.len();
        self.new_tab(ctx, None);
        if self.tabs.len() > tabs {
            let session = self.session();
            let shell = &crate::pty::SHELLS[session.kind];
            let input = crate::agents::launch_input(shell, dir.as_deref(), &agent.command);
            session.write(input.as_bytes());
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
        // El panel nuevo abre la misma shell, y se ajusta a su tamaño al dibujarse.
        let (grid, kind) = (self.session().grid, self.session().kind);
        match Session::new(ctx, grid, kind) {
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

    /// Barra lateral de pestañas:
    /// "+ Nueva pestaña" arriba y una fila por pestaña con un punto de estado y el nombre de la carpeta.
    /// Se puede ensanchar o angostar arrastrando su borde.
    fn tab_sidebar(&self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        let frame = egui::Frame::new()
            .fill(surface(theme().sidebar()))
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
                ui.painter().rect_filled(rect, 6.0, theme().bg);
            }
            let color = if new.hovered() {
                theme().fg
            } else {
                theme().ansi[8]
            };
            let text_pos = rect.left_center() + egui::vec2(10.0, 0.0);
            ui.painter().text(
                text_pos,
                egui::Align2::LEFT_CENTER,
                "+  Nueva pestaña",
                font.clone(),
                color,
            );
            // Clic derecho: elegir con qué shell.
            new.context_menu(|ui| {
                for &i in installed_shells() {
                    if ui.button(SHELLS[i].name).clicked() {
                        action = Some(Action::NewTab(Some(i)));
                        ui.close();
                    }
                }
                if !self.settings.agents.is_empty() {
                    ui.separator();
                }
                for (i, agent) in self.settings.agents.iter().enumerate() {
                    if ui.button(format!("Agente: {}", agent.name)).clicked() {
                        action = Some(Action::Agent(i));
                        ui.close();
                    }
                }
            });
            let tip = "Ctrl+Shift+T\nClic derecho: elegir la shell";
            if new.on_hover_text(tip).clicked() {
                action = Some(Action::NewTab(None));
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
                    painter.rect_filled(rect, 6.0, theme().bg);
                    let accent = egui::Rect::from_min_size(
                        rect.min + egui::vec2(0.0, 7.0),
                        egui::vec2(3.0, 16.0),
                    );
                    painter.rect_filled(accent, 1.5, theme().ansi[4]);
                } else if row.hovered() {
                    painter.rect_filled(rect, 6.0, theme().hover());
                }

                // Punto de estado: cómo terminó el último comando (o amarillo si algo corre).
                // Si un panel de la pestaña pide atención, el punto es azul y late.
                let center = rect.left_center() + egui::vec2(14.0, 0.0);
                let alert = tab
                    .panes
                    .leaves()
                    .into_iter()
                    .find_map(|s| s.attention.clone());
                if alert.is_some() {
                    let t = ui.input(|i| i.time) as f32;
                    let pulse = (t * 4.0).sin() * 0.5 + 0.5; // de 0 a 1 y vuelta
                    let ring = theme().ansi[4].gamma_multiply(0.35 * (1.0 - pulse));
                    painter.circle_filled(center, 4.0 + 4.0 * pulse, ring);
                    painter.circle_filled(center, 4.0, theme().ansi[4]);
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(50));
                } else {
                    let dot = session
                        .activity()
                        .map_or(theme().ansi[8], render::status_color);
                    painter.circle_filled(center, 3.5, dot);
                }

                // × a la derecha, solo al pasar el mouse por la fila.
                let close_rect = egui::Rect::from_center_size(
                    rect.right_center() - egui::vec2(14.0, 0.0),
                    egui::vec2(20.0, 20.0),
                );
                let close = ui.interact(close_rect, row.id.with("cerrar"), egui::Sense::click());

                // Nombre de la carpeta, recortado para que no pise la ×.
                let color = if active { theme().fg } else { theme().ansi[7] };
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
                    let x_color = if close.hovered() {
                        theme().fg
                    } else {
                        theme().ansi[8]
                    };
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
                let mut tip = match session.last_command() {
                    Some(command) => {
                        format!("Último comando: {command}\nClic del medio para cerrar")
                    }
                    None => "Clic del medio para cerrar".into(),
                };
                if let Some(alert) = &alert {
                    tip = format!("🔔 {alert}\n{tip}");
                }
                let row = row.on_hover_text(tip);
                if row.clicked() {
                    action = Some(Action::SelectTab(i));
                }
                if row.middle_clicked() {
                    action = Some(Action::CloseTab(i));
                }
                ui.add_space(2.0);
            }

            // Abajo de todo: el aviso de actualización, si hay una.
            let (text, next) = match self.updater.state() {
                UpdateState::Available(v) => (
                    format!("↑  Actualizar a {v}"),
                    Some(Action::InstallUpdate(v)),
                ),
                UpdateState::Installing(v) => (format!("Descargando {v}…"), None),
                UpdateState::Ready => ("↻  Reiniciar Orion".into(), Some(Action::RestartForUpdate)),
                _ => return,
            };
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                let (rect, row) =
                    ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::CLICK);
                if row.hovered() && next.is_some() {
                    ui.painter().rect_filled(rect, 6.0, theme().hover());
                }
                let pos = rect.left_center() + egui::vec2(10.0, 0.0);
                let color = theme().ansi[2];
                ui.painter()
                    .text(pos, egui::Align2::LEFT_CENTER, text, font.clone(), color);
                let tip = match next {
                    Some(Action::RestartForUpdate) => {
                        "Se cierran las pestañas y Orion abre la versión nueva"
                    }
                    Some(_) => "Descarga la versión nueva, la verifica y la deja lista",
                    None => "Puedes seguir usando Orion mientras tanto",
                };
                if row.on_hover_text(tip).clicked() {
                    action = next;
                }
            });
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
                return Some(Action::NewTab(None));
            }
            // Splits: D = a la derecha, E = hacia abajo. W cierra el panel.
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
            Action::NewTab(kind) => self.new_tab(ctx, kind),
            Action::Agent(i) => self.open_agent(ctx, i),
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
            Action::TogglePerf => {
                self.perf = if self.perf.is_some() {
                    None
                } else {
                    Some(Perf::new())
                };
            }
            Action::CheckUpdates => {
                self.update_asked = true;
                self.updater.check(ctx);
            }
            Action::InstallUpdate(version) => self.updater.install(ctx, version),
            Action::RestartForUpdate => match update::restart() {
                Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => self.show_toast(format!("No pude reiniciar: {e}")),
            },
            Action::Backdrop(i) => {
                let mut new = self.settings.clone();
                new.backdrop = crate::backdrop::BACKDROPS[i].0.into();
                self.change_settings(ctx, new);
            }
            Action::Theme(i) => {
                let mut new = self.settings.clone();
                new.theme = crate::colors::THEMES[i].name.into();
                self.change_settings(ctx, new);
            }
        }
    }

    /// Si guardaste config.toml desde otro editor, lo vuelve a leer y aplica los cambios.
    fn watch_config(&mut self, ctx: &egui::Context) {
        // egui duerme si nada pasa: le pedimos despertar para volver a mirar.
        ctx.request_repaint_after(CONFIG_POLL);
        if self.config_checked.elapsed() < CONFIG_POLL {
            return;
        }
        self.config_checked = Instant::now();
        let modified = config_modified();
        if modified == self.config_modified {
            return;
        }
        self.config_modified = modified;
        match reload_settings() {
            // Si es lo mismo que ya tenemos (ej. lo acaba de guardar el menú), no hay nada que avisar.
            Ok(new) if new == self.settings => {}
            Ok(new) => {
                self.apply_settings(ctx, new);
                self.show_toast("Configuración recargada");
            }
            // Mientras tenga errores se sigue usando la configuración anterior.
            Err(e) => self.show_toast(format!("Error en config.toml, {e}")),
        }
    }

    /// Avisos de este frame: los del panel que estás mirando se descartan (ya lo ves);
    /// los demás marcan su pestaña y panel. Si Orion no está en primer plano, parpadea en la barra de tareas.
    fn collect_alerts(&mut self, ctx: &egui::Context) {
        let (active, focused) = (self.active, self.focused);
        let mut new_alert = false;
        for (t, tab) in self.tabs.iter_mut().enumerate() {
            let focus = tab.focus;
            for session in tab.panes.leaves_mut() {
                let Some(alert) = session.alert.take() else {
                    continue;
                };
                if !(focused && t == active && session.id == focus) {
                    session.attention = Some(alert);
                    new_alert = true;
                }
            }
        }
        // Lo que estás mirando ya no pide atención.
        if focused {
            self.session().attention = None;
        }
        if new_alert && !focused {
            let blink = egui::UserAttentionType::Informational;
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(blink));
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
        // Con Mica o acrílico no: los paneles semitransparentes se oscurecerían de más al pintarse encima.
        if opaque() {
            ui.painter().rect_filled(area, 0.0, theme().sidebar());
        }
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
            if session.attention.is_some() {
                // Un panel de al lado pide atención: borde azul.
                let stroke = egui::Stroke::new(2.0, theme().ansi[4]);
                ui.painter()
                    .rect_stroke(rect.shrink(1.0), 4.0, stroke, egui::StrokeKind::Inside);
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

/// La shell de config.toml, o la primera instalada si esa no está.
fn default_shell(settings: &Settings) -> usize {
    usable_shell(find_shell(&settings.shell).unwrap_or(0))
}

/// Qué pestaña queda activa al cerrar la `closed`, si quedan `left`.
/// La activa sigue siendo la misma pestaña: si cerraste una de su izquierda, su índice baja uno.
/// Si cerraste la activa, queda la de su derecha (o la de su izquierda si era la última).
fn active_after_close(active: usize, closed: usize, left: usize) -> usize {
    let active = if closed < active { active - 1 } else { active };
    active.min(left.saturating_sub(1))
}

impl eframe::App for Orion {
    // Lo que no pintamos queda transparente: así se ve Mica o el acrílico.
    // Sin esos fondos, la barra lateral y los paneles cubren toda la ventana con colores opacos.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    // logic corre antes de cada ui y también con la ventana oculta: así ningún canal se acumula.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Todos los paneles de todas las pestañas: los de atrás siguen recibiendo su salida.
        let mut pending = false;
        let mut copied = None;
        for tab in &mut self.tabs {
            for session in tab.panes.leaves_mut() {
                pending |= session.process_output();
                copied = session.take_copied().or(copied);
            }
        }
        // Un programa copió algo (OSC 52, ej. "copiar respuesta" en un agente): al portapapeles.
        if let Some(text) = copied {
            let lines = text.lines().count().max(1);
            ctx.copy_text(text);
            let what = if lines == 1 {
                "1 línea".into()
            } else {
                format!("{lines} líneas")
            };
            self.show_toast(format!("Copiado al portapapeles ({what})"));
        }
        if pending {
            ctx.request_repaint(); // quedó salida: seguir en el próximo frame
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
        self.watch_config(ctx);
        self.collect_alerts(ctx);
        // Resultado de buscar actualizaciones a mano.
        match self.updater.state() {
            UpdateState::UpToDate if self.update_asked => {
                self.show_toast(format!("Orion está al día ({})", update::CURRENT));
                self.update_asked = false;
            }
            UpdateState::CheckFailed(e) if self.update_asked => {
                self.show_toast(format!("Actualización: {e}"));
                self.update_asked = false;
                self.updater.dismiss();
            }
            // Instalar siempre lo pediste con un clic: el error se muestra siempre.
            UpdateState::Failed(e) => {
                self.show_toast(format!("Actualización: {e}"));
                self.updater.dismiss();
            }
            _ => {}
        }
        // El título de la ventana sigue al del panel con foco.
        let title = format!("{} — Orion", self.session().title());
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
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

        // Archivos arrastrados: las fuentes se instalan; el resto se escribe como rutas, como un pegado.
        let dropped: Vec<PathBuf> = ui.input(|i| {
            let files = i.raw.dropped_files.iter();
            files.map(|f| f.path().to_path_buf()).collect()
        });
        let (fonts, others): (Vec<_>, Vec<_>) = dropped
            .into_iter()
            .partition(|p| is_font_file(&p.to_string_lossy()));
        for path in fonts {
            self.font_dropped(&ctx, &path);
        }
        if !others.is_empty() {
            let session = self.session();
            let shell = &crate::pty::SHELLS[session.kind];
            let paths: Vec<String> = others
                .iter()
                .map(|p| shell.quote_path(&p.to_string_lossy()))
                .collect();
            let bracketed = session.term.mode().contains(TermMode::BRACKETED_PASTE);
            session.write(&crate::input::paste_bytes(
                &format!("{} ", paths.join(" ")),
                bracketed,
            ));
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

        // Ctrl+, abre o cierra la configuración; Escape la cierra.
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
        if let Some(perf) = &mut self.perf {
            // cpu_usage es del frame anterior (este todavía no terminó).
            perf.frame(frame.info().cpu_usage.unwrap_or(0.0));
            let text = format!("{:.1} ms/frame · {} frames/s", perf.avg_ms, perf.fps);
            let font = egui::FontId::monospace(12.0);
            let pos = area.right_top() + egui::vec2(-10.0, 6.0);
            let color = theme().ansi[3];
            ui.painter()
                .text(pos, egui::Align2::RIGHT_TOP, text, font, color);
            // Para que el contador se actualice aunque no pase nada.
            ui.ctx().request_repaint_after(Duration::from_millis(500));
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
