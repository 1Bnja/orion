//! Una pestaña: su shell, su grilla, sus bloques y lo que pasa con el teclado adentro.
//!
//! Cada pestaña es independiente: tiene su propio PowerShell, su historial y sus bloques.
//! Las que no se ven siguen recibiendo la salida de su shell (ej. un build que corre en otra pestaña).

use super::MAC;
use super::render::BlockAction;
use crate::blocks::{
    Blocks, Mark, MarkScanner, Piece, Status, grid_line_of, neighbor_block, scroll_to_show,
};
use crate::grid::term_size;
use crate::input::{key_bytes, paste_bytes};
use crate::pty::{Error, Replies, SHELLS, Shell, pty_size, spawn_reader};
use alacritty_terminal::Term;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle, Processor};
use eframe::egui;
use std::cell::{Cell, RefCell};
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
    /// Campana (BEL) del Term; se lee en `process_output`.
    bell: Rc<Cell<bool>>,
    /// Texto que un programa copió (OSC 52); la app lo lleva al portapapeles con `take_copied`.
    copied: Rc<RefCell<Option<String>>>,
    /// Algo pidió atención en este frame (un aviso, la campana o un comando largo que terminó).
    /// La app decide si lo estás mirando; si no, lo pasa a `attention`.
    pub(super) alert: Option<String>,
    /// Pide atención y todavía no lo miraste: marca la pestaña y el panel. El texto va en el tooltip.
    pub(super) attention: Option<String>,
    /// Carpeta actual, si la shell la informa (OSC 9;9). Para abrir rutas relativas.
    pub(super) cwd: Option<std::path::PathBuf>,
    /// El agente que Orion escribió en este panel, seguido por las marcas de la shell.
    pub(super) agent: Option<AgentWatch>,
}

/// Un comando que terminó en una pestaña que no miras avisa solo si tardó al menos esto
/// (que `dir` termine no es noticia; que termine un build de 2 minutos, sí).
const LONG_COMMAND: Duration = Duration::from_secs(10);

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
        let bell = Rc::new(Cell::new(false));
        let copied = Rc::new(RefCell::new(None));
        let replies = Replies {
            writer: shell.writer.clone(),
            title: title.clone(),
            bell: bell.clone(),
            copied: copied.clone(),
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
            bell,
            copied,
            alert: None,
            attention: None,
            cwd: None,
            agent: None,
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
                    Piece::Mark(Mark::Notify(text)) => self.alert = Some(text),
                    Piece::Mark(Mark::Cwd(dir)) => self.cwd = Some(dir.into()),
                    // La marca se aplica al llegar: el cursor está justo donde la shell la puso.
                    Piece::Mark(mark) => {
                        if let Some(agent) = &mut self.agent {
                            agent.on_mark(&mark);
                        }
                        let finished = matches!(mark, Mark::Finished(_));
                        self.blocks.on_mark(mark, &self.term);
                        if let Some(block) = self.blocks.list.last().filter(|_| finished)
                            && block.duration.is_some_and(|d| d >= LONG_COMMAND)
                        {
                            self.alert = Some(format!("Terminó: {}", block.command));
                        }
                    }
                }
            }
            if self.bell.take() {
                self.alert.get_or_insert_with(|| "Pide atención".into());
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

    /// Lo que un programa copió desde el último frame (OSC 52), si copió algo.
    pub(super) fn take_copied(&self) -> Option<String> {
        self.copied.borrow_mut().take()
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

    /// Ctrl+C / Ctrl+Shift+C (Cmd+C en Mac): copia la selección; si no hay, la salida del bloque marcado.
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
                // Ctrl+↑ / Ctrl+↓ (Cmd en Mac): saltar entre bloques.
                egui::Event::Key {
                    key: key @ (egui::Key::ArrowUp | egui::Key::ArrowDown),
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command && !modifiers.shift && !modifiers.alt && !alt_screen => {
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
                // egui convierte Ctrl+C (Cmd+C en Mac) en Copy. Si hay selección (o un bloque
                // marcado) copia; si no, en Windows es la interrupción de siempre (ETX).
                // En Mac, Cmd+C solo copia: Ctrl+C llega como tecla e interrumpe por key_bytes.
                egui::Event::Copy => {
                    if self.copy(ui.ctx()) || MAC {
                        continue;
                    }
                    vec![0x03]
                }
                // Cmd+X en Mac no es nada para la shell (Ctrl+X llega como tecla).
                egui::Event::Cut if MAC => continue,
                egui::Event::Cut => vec![0x18],
                // Ctrl+Shift+C / Ctrl+Shift+V: copiar y pegar sin chocar con Ctrl+C (interrumpir).
                // En Mac no hace falta (Cmd+C / Cmd+V): ahí Control es todo de la terminal.
                egui::Event::Key {
                    key: egui::Key::C,
                    pressed: true,
                    modifiers,
                    ..
                } if !MAC && modifiers.ctrl && modifiers.shift => {
                    self.copy(ui.ctx());
                    continue;
                }
                egui::Event::Key {
                    key: egui::Key::V,
                    pressed: true,
                    modifiers,
                    ..
                } if !MAC && modifiers.ctrl && modifiers.shift => {
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

// Usan PowerShell de verdad: por ahora, solo en Windows.
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::Duration;

    /// De punta a punta, como en la app: un PowerShell real copia con OSC 52 y la sesión lo recibe.
    #[test]
    fn a_real_shell_can_copy_through_the_session() {
        let ctx = egui::Context::default();
        let mut session = Session::new(&ctx, (80, 24), crate::pty::usable_shell(0)).unwrap();
        let wait = |session: &mut Session, done: &dyn Fn(&Session) -> bool| {
            let start = Instant::now();
            while !done(session) {
                assert!(
                    start.elapsed() < Duration::from_secs(15),
                    "se acabó el tiempo"
                );
                session.process_output();
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        // Esperar el primer prompt (hay un bloque abierto).
        wait(&mut session, &|s| !s.blocks.list.is_empty());
        session.write(b"[Console]::Write(\"`e]52;c;aG9sYQ==`a\")\r");
        let mut copied = None;
        let start = Instant::now();
        while copied.is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "no llegó lo copiado"
            );
            session.process_output();
            copied = session.take_copied();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(copied.as_deref(), Some("hola"));
        let _ = session.shell.child.kill();
    }

    /// Un agente se escribe apenas se abre la pestaña, antes del primer prompt: no se pierde,
    /// corre en su carpeta y la sesión se entera de la carpeta (OSC 9;9).
    #[test]
    fn an_agent_typed_before_the_prompt_runs_in_its_folder() {
        let dir = std::env::temp_dir().join(format!("orion agente {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let kind = crate::pty::usable_shell(0);
        let mut session = Session::new(&ctx, (80, 24), kind).unwrap();
        let shell = &crate::pty::SHELLS[kind];
        let command = "'ok' | Set-Content listo.txt";
        session.write(crate::agents::launch_input(shell, Some(&dir), command).as_bytes());
        session.agent = Some(AgentWatch::new(1)); // antes va el `cd`
        let start = Instant::now();
        // Comparadas ya resueltas: TEMP puede venir con nombres cortos (RUNNER~1) y la shell da los largos.
        let same =
            |a: &std::path::Path| std::fs::canonicalize(a).ok() == std::fs::canonicalize(&dir).ok();
        let done = |s: &Session| s.agent.map(|a| a.status) == Some(AgentStatus::Done(Some(0)));
        while !dir.join("listo.txt").exists()
            || !session.cwd.as_deref().is_some_and(same)
            || !done(&session)
        {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "cwd: {:?}, agente: {:?}",
                session.cwd,
                session.agent
            );
            session.process_output();
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = session.shell.child.kill();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Cómo va un agente.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentStatus {
    /// Todavía no arrancó su comando.
    Starting,
    Running,
    /// Terminó, con su código si la shell lo informó.
    Done(Option<i32>),
}

/// Sigue el comando de un agente por las marcas OSC 133, no por la pantalla: un programa de
/// pantalla completa o un `clear` borran lo que se ve, pero las marcas llegan igual. El comando
/// del agente es el que arranca (marca C) después de saltar `skip` comandos: el `cd` que Orion
/// escribe antes. Solo PowerShell y zsh mandan marcas: en cmd y WSL se queda en `Starting`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AgentWatch {
    skip: usize,
    pub(super) status: AgentStatus,
}

impl AgentWatch {
    pub(super) fn new(skip: usize) -> Self {
        Self {
            skip,
            status: AgentStatus::Starting,
        }
    }

    fn on_mark(&mut self, mark: &Mark) {
        match (mark, self.status) {
            (Mark::OutputStart, AgentStatus::Starting) if self.skip > 0 => self.skip -= 1,
            (Mark::OutputStart, AgentStatus::Starting) => self.status = AgentStatus::Running,
            (Mark::Finished(code), AgentStatus::Running) => self.status = AgentStatus::Done(*code),
            _ => {}
        }
    }
}

#[cfg(test)]
mod agent_tests {
    use super::*;

    #[test]
    fn the_agent_is_the_command_after_the_cd() {
        let mut watch = AgentWatch::new(1);
        let mut feed = |marks: &[Mark]| {
            for mark in marks {
                watch.on_mark(mark);
            }
            watch.status
        };
        let prompt = [Mark::PromptStart, Mark::CommandStart];
        assert_eq!(feed(&prompt), AgentStatus::Starting);
        // El `cd`: arranca y termina, pero no es el agente.
        let cd = [Mark::OutputStart, Mark::Finished(Some(0))];
        assert_eq!(feed(&cd), AgentStatus::Starting);
        // Un Enter vacío (D sin C) tampoco.
        assert_eq!(feed(&prompt), AgentStatus::Starting);
        assert_eq!(feed(&[Mark::Finished(None)]), AgentStatus::Starting);
        assert_eq!(feed(&prompt), AgentStatus::Starting);
        assert_eq!(feed(&[Mark::OutputStart]), AgentStatus::Running);
        assert_eq!(feed(&[Mark::Finished(Some(2))]), AgentStatus::Done(Some(2)));
        // Lo que se escriba después no cambia cómo terminó.
        assert_eq!(feed(&prompt), AgentStatus::Done(Some(2)));
        let again = [Mark::OutputStart, Mark::Finished(Some(0))];
        assert_eq!(feed(&again), AgentStatus::Done(Some(2)));
    }
}
