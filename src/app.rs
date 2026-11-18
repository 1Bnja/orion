//! La app: estado de Orion y lo que pasa en cada frame (shell → pantalla, teclado → shell).
//!
//! `menu` y `render` son submódulos: al ser hijos de `app`, pueden usar los campos privados de `Orion`.

mod menu;
mod render;

use crate::blocks::{Blocks, MarkScanner, Piece};
use crate::config::Settings;
use crate::fonts::{apply_font, is_font_file};
use crate::grid::term_size;
use crate::input::{key_bytes, paste_bytes};
use crate::pty::{Error, Replies, Shell, spawn_reader};
use alacritty_terminal::Term;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle, Processor};
use eframe::egui;
use menu::SettingsMenu;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

pub(crate) struct Orion {
    shell: Shell,
    rx: Receiver<Vec<u8>>,
    /// Parser VT: convierte bytes en acciones (escribir, mover cursor, borrar…) que aplica sobre el Term.
    /// Guarda estado entre trozos: una secuencia o un carácter UTF-8 puede llegar partido en dos lecturas.
    processor: Processor,
    /// La grilla de celdas + cursor + historial. Es el "estado de la pantalla".
    term: Term<Replies>,
    /// Columnas y filas actuales. Se compara cada frame para redimensionar solo si cambia.
    grid: (u16, u16),
    /// Desde cuándo cuenta el parpadeo. Se reinicia al teclear: el cursor queda visible mientras escribes.
    blink_epoch: Instant,
    /// Fracción de línea de scroll pendiente (ver `scroll_lines`).
    scroll_acc: f32,
    settings: Settings,
    menu: SettingsMenu,
    /// Botón apretado mientras la app recibe el mouse (para informar arrastres).
    mouse_button: Option<u8>,
    /// Última celda informada a la app: solo avisamos movimientos cuando cambia.
    mouse_cell: Option<(usize, usize)>,
    /// Si la ventana tenía el foco en el frame anterior (para avisar cambios a la app).
    focused: bool,
    /// Separa las marcas OSC 133 del resto de la salida.
    marks: MarkScanner,
    /// Un bloque por comando: lo que escribiste, dónde está su salida, código y duración.
    blocks: Blocks,
}

impl Orion {
    /// Prepara la app cuando egui ya creó la ventana: fuente, hilo lector del PTY y la grilla.
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        shell: Shell,
        settings: Settings,
        grid: (u16, u16),
    ) -> Result<Self, Error> {
        apply_font(&cc.egui_ctx, &settings.font.family);
        // El Context de egui se puede clonar y usar desde otro hilo para pedir un repaint.
        let ctx = cc.egui_ctx.clone();
        let rx = spawn_reader(&*shell.master, move || ctx.request_repaint())?;
        // Cursor por defecto: bloque que parpadea. Las apps pueden cambiarlo con ESC[n q (DECSCUSR).
        let config = Config {
            default_cursor_style: CursorStyle {
                shape: CursorShape::Block,
                blinking: true,
            },
            ..Config::default()
        };
        let term = Term::new(config, &term_size(grid), Replies(shell.writer.clone()));
        Ok(Self {
            shell,
            rx,
            processor: Processor::new(),
            term,
            grid,
            blink_epoch: Instant::now(),
            scroll_acc: 0.0,
            settings,
            menu: SettingsMenu::default(),
            mouse_button: None,
            mouse_cell: None,
            focused: true,
            marks: MarkScanner::default(),
            blocks: Blocks::default(),
        })
    }
}

impl eframe::App for Orion {
    // logic corre antes de cada ui y también con la ventana oculta: así el canal nunca se acumula.
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // try_iter vacía el canal sin bloquear el frame.
        for chunk in self.rx.try_iter() {
            for piece in self.marks.feed(&chunk) {
                match piece {
                    Piece::Data(bytes) => self.processor.advance(&mut self.term, &bytes),
                    // La marca se aplica al llegar: el cursor está justo donde la shell la puso.
                    Piece::Mark(mark) => self.blocks.on_mark(mark, &self.term),
                }
            }
            self.blocks.after_output(&self.term);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Si la app lo pidió (ESC[?1004h, ej. vim), avisarle cuando la ventana gana o pierde el foco.
        let focused = ui.input(|i| i.focused);
        if focused != self.focused {
            self.focused = focused;
            if self.term.mode().contains(TermMode::FOCUS_IN_OUT) {
                let report: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
                let _ = self.shell.writer.borrow_mut().write_all(report);
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
            self.send_input(ui);
        }
        // Con el menú abierto, el teclado es para el menú (ej. el buscador), no para la shell.

        self.draw_terminal(ui);
        self.settings_window(&ctx);
    }
}

impl Orion {
    /// Copia la selección al portapapeles y la quita (señal visual de que se copió).
    /// Devuelve false si no había nada seleccionado.
    pub(crate) fn copy_selection(&mut self, ctx: &egui::Context) -> bool {
        match self.term.selection_to_string() {
            Some(text) if !text.is_empty() => {
                ctx.copy_text(text);
                self.term.selection = None;
                true
            }
            _ => false,
        }
    }

    /// Teclado y portapapeles → shell.
    pub(crate) fn send_input(&mut self, ui: &egui::Ui) {
        let bracketed = self.term.mode().contains(TermMode::BRACKETED_PASTE);
        let app_cursor = self.term.mode().contains(TermMode::APP_CURSOR);
        let alt = ui.input(|i| i.modifiers.alt);
        // El teclado va en ui y no en logic: con la ventana oculta, logic vería otra vez los eventos del último frame.
        for event in ui.input(|i| i.events.clone()) {
            let bytes = match event {
                // Alt+letra también llega como texto; ya la manda key_bytes con su ESC adelante.
                egui::Event::Text(t)
                    if alt && t.len() == 1 && t.as_bytes()[0].is_ascii_alphabetic() =>
                {
                    continue;
                }
                egui::Event::Text(t) => t.into_bytes(),
                egui::Event::Paste(t) => paste_bytes(&t, bracketed),
                // egui convierte Ctrl+C en Copy. Como en Windows Terminal: si hay selección copia,
                // si no, es la interrupción de siempre (ETX).
                egui::Event::Copy => {
                    if self.copy_selection(ui.ctx()) {
                        continue;
                    }
                    vec![0x03]
                }
                egui::Event::Cut => vec![0x18],
                // Ctrl+Shift+C / Ctrl+Shift+V: los atajos de Ghostty/Warp/Linux.
                egui::Event::Key {
                    key: egui::Key::C,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.ctrl && modifiers.shift => {
                    self.copy_selection(ui.ctx());
                    continue;
                }
                egui::Event::Key {
                    key: egui::Key::V,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.ctrl && modifiers.shift => {
                    // egui lee el portapapeles y nos lo devuelve como Event::Paste en un próximo frame.
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    continue;
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => match key_bytes(key, modifiers, app_cursor) {
                    Some(b) => b,
                    None => continue,
                },
                _ => continue,
            };
            // Si la shell ya terminó, escribir falla; no hay nada que hacer con el error.
            let _ = self.shell.writer.borrow_mut().write_all(&bytes);
            self.blink_epoch = Instant::now();
            // Si estabas mirando el historial, escribir te devuelve abajo, a donde está el prompt.
            self.term.scroll_display(Scroll::Bottom);
            self.term.selection = None;
        }
    }
}
