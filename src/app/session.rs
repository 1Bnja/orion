//! Una pestaña: su shell, su grilla, sus bloques y lo que pasa con el teclado adentro.
//!
//! Cada pestaña es independiente: tiene su propio PowerShell, su historial y sus bloques.
//! Las que no se ven siguen recibiendo la salida de su shell (ej. un build que corre en otra pestaña).

use super::render::BlockAction;
use crate::blocks::{
    Blocks, MarkScanner, Piece, Status, grid_line_of, neighbor_block, scroll_to_show,
};
use crate::grid::term_size;
use crate::input::{key_bytes, paste_bytes};
use crate::pty::{Error, Replies, SHELLS, Shell, pty_size, spawn_reader};
use alacritty_terminal::Term;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle, Processor};
use eframe::egui;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

pub(super) struct Session {
    pub(super) shell: Shell,
    rx: Receiver<Vec<u8>>,
    /// Parser VT: convierte bytes en acciones (escribir, mover cursor, borrar…) que aplica sobre el Term.
    /// Guarda estado entre trozos: una secuencia o un carácter UTF-8 puede llegar partido en dos lecturas.
    processor: Processor,
    /// La grilla de celdas + cursor + historial. Es el "estado de la pantalla".
    pub(super) term: Term<Replies>,
    /// Columnas y filas actuales. Se compara cada frame para redimensionar solo si cambia.
    pub(super) grid: (u16, u16),
    /// Desde cuándo cuenta el parpadeo. Se reinicia al teclear: el cursor queda visible mientras escribes.
    pub(super) blink_epoch: Instant,
    /// Fracción de línea de scroll pendiente (ver `scroll_lines`).
    pub(super) scroll_acc: f32,
    /// Botón apretado mientras la app recibe el mouse (para informar arrastres).
    pub(super) mouse_button: Option<u8>,
    /// Última celda informada a la app: solo avisamos movimientos cuando cambia.
    pub(super) mouse_cell: Option<(usize, usize)>,
    /// Separa las marcas OSC 133 del resto de la salida.
    marks: MarkScanner,
    /// Un bloque por comando: lo que escribiste, dónde está su salida, código y duración.
    pub(super) blocks: Blocks,
    /// Bloque sobre el que se hizo clic derecho (el menú lo usa mientras está abierto).
    pub(super) context_block: Option<usize>,
    /// Bloque marcado al navegar con Ctrl+↑/↓ (se resalta y Ctrl+C copia su salida).
    pub(super) marked_block: Option<usize>,
    /// Título que pone la shell (OSC 0). La integración pone el nombre de la carpeta actual.
    title: Rc<RefCell<Option<String>>>,
    /// Aviso pendiente ("Salida copiada"); la app lo toma y lo muestra.
    pub(super) toast: Option<String>,
    /// Qué shell corre (índice en `SHELLS`). Un split del panel abre la misma.
    pub(super) kind: usize,
    /// Identifica el panel (para el foco entre splits). Único mientras corre la app.
    pub(super) id: u64,
    /// Se pidió dividir este panel desde el menú de clic derecho: Some(true) = a la derecha.
    pub(super) split_request: Option<bool>,
}

/// Tiempo máximo por frame para procesar salida de la shell. El resto queda para el próximo frame.
/// A ~60 MB/s son unos 600 KB por frame: de sobra para cualquier uso normal.
const OUTPUT_BUDGET: Duration = Duration::from_millis(10);

/// Próximo `id` de panel.
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

impl super::panes::Leaf for Session {
    fn id(&self) -> u64 {
        self.id
    }
}

impl Session {
    /// Lanza la shell `kind` (índice en `SHELLS`) del tamaño `grid`.
    /// PowerShell arranca con la integración de Orion: marca cada prompt y comando con OSC 133.
    pub(super) fn new(ctx: &egui::Context, grid: (u16, u16), kind: usize) -> Result<Self, Error> {
        let shell = SHELLS[kind].spawn(pty_size(grid))?;
        // El Context de egui se puede clonar y usar desde otro hilo para pedir un repaint.
        let ctx = ctx.clone();
        let rx = spawn_reader(&*shell.master, move || ctx.request_repaint())?;
        // Cursor por defecto: bloque que parpadea. Las apps pueden cambiarlo con ESC[n q (DECSCUSR).
        let config = Config {
            default_cursor_style: CursorStyle {
                shape: CursorShape::Block,
                blinking: true,
            },
            ..Config::default()
        };
        let title = Rc::new(RefCell::new(None));
        let replies = Replies {
            writer: shell.writer.clone(),
            title: title.clone(),
        };
        let term = Term::new(config, &term_size(grid), replies);
        Ok(Self {
            shell,
            rx,
            processor: Processor::new(),
            term,
            grid,
            blink_epoch: Instant::now(),
            scroll_acc: 0.0,
            mouse_button: None,
            mouse_cell: None,
            marks: MarkScanner::default(),
            blocks: Blocks::default(),
            context_block: None,
            marked_block: None,
            title,
            toast: None,
            kind,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            split_request: None,
        })
    }

    /// El nombre que se muestra en la pestaña: el que puso la shell, o el de la shell.
    pub(super) fn title(&self) -> String {
        let title = self.title.borrow().clone();
        title.unwrap_or_else(|| SHELLS[self.kind].name.into())
    }

    /// Estado para el punto de la pestaña: si algo corre, o cómo terminó el último comando.
    /// None = todavía no se ejecutó nada.
    pub(super) fn activity(&self) -> Option<Status> {
        let mut statuses = self.blocks.list.iter().rev().map(|b| b.status());
        statuses.find(|s| !matches!(s, Status::Editing))
    }

    /// El último comando que se ejecutó (para el tooltip de la pestaña).
    pub(super) fn last_command(&self) -> Option<&str> {
        let mut commands = self.blocks.list.iter().rev().map(|b| b.command.as_str());
        commands.find(|c| !c.is_empty())
    }

    /// Procesa lo que la shell imprimió desde el último frame (sin bloquear).
    /// Con mucha salida (un `cat` de un archivo enorme) para a los `OUTPUT_BUDGET`:
    /// así la ventana se sigue dibujando y Ctrl+C llega. Devuelve true si quedó salida pendiente.
    pub(super) fn process_output(&mut self) -> bool {
        let start = Instant::now();
        // try_recv no bloquea: si no hay nada (o el lector terminó), se sale del while.
        while let Ok(chunk) = self.rx.try_recv() {
            for piece in self.marks.feed(&chunk) {
                match piece {
                    Piece::Data(bytes) => self.processor.advance(&mut self.term, &bytes),
                    // La marca se aplica al llegar: el cursor está justo donde la shell la puso.
                    Piece::Mark(mark) => self.blocks.on_mark(mark, &self.term),
                }
            }
            self.blocks.after_output(&self.term);
            // Un `cls` vacía la lista: la marca no puede quedar apuntando a un bloque que ya no existe.
            if self
                .marked_block
                .is_some_and(|i| i >= self.blocks.list.len())
            {
                self.marked_block = None;
            }
            if start.elapsed() > OUTPUT_BUDGET {
                return true;
            }
        }
        false
    }

    /// ¿La shell terminó? (escribiste `exit`, o se cerró sola).
    /// Se pregunta al proceso: con ConPTY, la lectura no termina sola aunque la shell haya salido.
    pub(super) fn exited(&mut self) -> bool {
        matches!(self.shell.child.try_wait(), Ok(Some(_)))
    }

    /// Manda bytes a la shell. Si ya terminó, escribir falla; no hay nada que hacer con el error.
    pub(super) fn write(&self, bytes: &[u8]) {
        let _ = self.shell.writer.borrow_mut().write_all(bytes);
    }

    /// Copia la selección al portapapeles y la quita (señal visual de que se copió).
    /// Devuelve false si no había nada seleccionado.
    pub(super) fn copy_selection(&mut self, ctx: &egui::Context) -> bool {
        match self.term.selection_to_string() {
            Some(text) if !text.is_empty() => {
                ctx.copy_text(text);
                self.term.selection = None;
                true
            }
            _ => false,
        }
    }

    /// Ctrl+C / Ctrl+Shift+C: copia la selección; si no hay, la salida del bloque marcado.
    /// Devuelve false si no había nada que copiar.
    fn copy(&mut self, ctx: &egui::Context) -> bool {
        if self.copy_selection(ctx) {
            return true;
        }
        let Some(index) = self.marked_block else {
            return false;
        };
        self.apply_block_action(ctx, BlockAction::CopyOutput(index));
        true
    }

    /// Ctrl+↑ / Ctrl+↓: marca el bloque anterior o el siguiente y mueve la vista para que se vea.
    /// Pasado el último, se quita la marca y se vuelve abajo, al prompt.
    fn jump_to_block(&mut self, up: bool) {
        let done: Vec<usize> = (0..self.blocks.list.len())
            .filter(|&i| matches!(self.blocks.list[i].status(), Status::Done(..)))
            .collect();
        // Si la marca apunta a un bloque que ya no existe (por un `cls`), se empieza de cero.
        let marked = self.marked_block.filter(|&i| i < self.blocks.list.len());
        self.marked_block = neighbor_block(&done, marked, up);
        let Some(index) = self.marked_block else {
            self.term.scroll_display(Scroll::Bottom);
            return;
        };
        let prompt = self.blocks.list[index].prompt;
        let Some(line) = grid_line_of(&self.term, prompt) else {
            return; // ya no está en el historial
        };
        let grid = self.term.grid();
        let (offset, screen) = (grid.display_offset(), grid.screen_lines());
        if let Some(target) = scroll_to_show(line.0, offset, screen, grid.history_size()) {
            let delta = target as i32 - offset as i32;
            self.term.scroll_display(Scroll::Delta(delta));
        }
    }

    /// Teclado y portapapeles → shell.
    pub(super) fn send_input(&mut self, ui: &egui::Ui) {
        let bracketed = self.term.mode().contains(TermMode::BRACKETED_PASTE);
        let app_cursor = self.term.mode().contains(TermMode::APP_CURSOR);
        let alt = ui.input(|i| i.modifiers.alt);
        // En apps de pantalla completa (vim, micro) no hay bloques: Ctrl+↑/↓ siguen yendo a la app.
        let alt_screen = self.term.mode().contains(TermMode::ALT_SCREEN);
        // El teclado va en ui y no en logic: con la ventana oculta, logic vería otra vez los eventos del último frame.
        for event in ui.input(|i| i.events.clone()) {
            let bytes = match event {
                // Ctrl+↑ / Ctrl+↓: saltar entre bloques.
                egui::Event::Key {
                    key: key @ (egui::Key::ArrowUp | egui::Key::ArrowDown),
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.ctrl && !modifiers.shift && !modifiers.alt && !alt_screen => {
                    self.jump_to_block(key == egui::Key::ArrowUp);
                    continue;
                }
                // Escape con un bloque marcado: quitar la marca y volver al prompt (no se manda a la shell).
                egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                } if self.marked_block.is_some() => {
                    self.marked_block = None;
                    self.term.scroll_display(Scroll::Bottom);
                    continue;
                }
                // Alt+letra también llega como texto; ya la manda key_bytes con su ESC adelante.
                // (Alt+número cambia de pestaña: tampoco se escribe.)
                egui::Event::Text(t)
                    if alt && t.len() == 1 && t.as_bytes()[0].is_ascii_alphanumeric() =>
                {
                    continue;
                }
                egui::Event::Text(t) => t.into_bytes(),
                egui::Event::Paste(t) => paste_bytes(&t, bracketed),
                // egui convierte Ctrl+C en Copy. Como en Windows Terminal: si hay selección (o un bloque
                // marcado) copia; si no, es la interrupción de siempre (ETX).
                egui::Event::Copy => {
                    if self.copy(ui.ctx()) {
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
                    self.copy(ui.ctx());
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
            self.write(&bytes);
            self.blink_epoch = Instant::now();
            // Si estabas mirando el historial, escribir te devuelve abajo, a donde está el prompt.
            self.term.scroll_display(Scroll::Bottom);
            self.term.selection = None;
            self.marked_block = None; // escribir vuelve al modo normal
        }
    }
}
