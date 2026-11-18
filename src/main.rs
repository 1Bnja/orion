use alacritty_terminal::{
    Term,
    event::{Event as TermEvent, EventListener},
    grid::Dimensions,
    term::{Config, cell::Flags, test::TermSize},
    vte::ansi::{CursorShape, CursorStyle, Processor},
};
use eframe::egui;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Medio ciclo de parpadeo (encendido o apagado), el mismo valor que usa Windows por defecto.
const BLINK: Duration = Duration::from_millis(530);

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Input hacia la shell. Compartido: escriben el teclado (UI) y las respuestas automáticas del Term.
/// ponytail: Rc<RefCell> porque todo corre en el hilo de la UI; pasaría a Arc<Mutex> si otro hilo escribiera.
type PtyWriter = Rc<RefCell<Box<dyn Write + Send>>>;

/// Una shell corriendo dentro de un pseudo-terminal (ConPTY en Windows).
#[allow(dead_code)] // ponytail: child solo se usa en tests hasta manejar el cierre de la shell.
struct Shell {
    /// Lado "terminal" del PTY: leemos lo que la shell imprime y le avisamos cambios de tamaño.
    master: Box<dyn MasterPty + Send>,
    writer: PtyWriter,
    child: Box<dyn Child + Send + Sync>,
}

/// Lanza pwsh (o powershell.exe si no está) con `args` extra.
///
/// Ojo: ConPTY arranca preguntando la posición del cursor (ESC[6n) y no lanza la shell
/// hasta recibir respuesta. En la app la responde el Term; quien use esto sin Term debe responderla.
fn spawn_shell(size: PtySize, args: &[&str]) -> Result<Shell, Error> {
    let pair = native_pty_system().openpty(size)?;
    let mut last_err = None;
    for program in ["pwsh.exe", "powershell.exe"] {
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        match pair.slave.spawn_command(cmd) {
            // ponytail: el slave se suelta al salir de la función; la shell ya tiene su copia.
            Ok(child) => {
                let writer = Rc::new(RefCell::new(pair.master.take_writer()?));
                return Ok(Shell {
                    master: pair.master,
                    writer,
                    child,
                });
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap().into())
}

/// Lee el PTY en un hilo propio y manda cada trozo de bytes por un canal.
/// `read` bloquea, por eso no puede ir en el hilo de la UI: congelaría la ventana.
/// `notify` se llama tras cada envío; la UI lo usa para despertarse y repintar.
fn spawn_reader(
    master: &dyn MasterPty,
    notify: impl Fn() + Send + 'static,
) -> Result<Receiver<Vec<u8>>, Error> {
    let mut reader = master.try_clone_reader()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        // Termina cuando la shell se cierra (read da 0 o error) o cuando la UI soltó el Receiver.
        while let Ok(n @ 1..) = reader.read(&mut buf) {
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
            notify();
        }
    });
    Ok(rx)
}

/// Recibe los avisos del Term. Solo nos importa `PtyWrite`: respuestas que la terminal
/// debe mandarle a la shell, como la posición del cursor cuando la shell la pregunta (ESC[6n).
struct Replies(PtyWriter);

impl EventListener for Replies {
    fn send_event(&self, event: TermEvent) {
        if let TermEvent::PtyWrite(text) = event {
            let _ = self.0.borrow_mut().write_all(text.as_bytes());
        }
    }
}

/// Texto visible del Term, un String por fila (con los espacios del final incluidos).
fn screen_text<T: EventListener>(term: &Term<T>) -> Vec<String> {
    let content = term.renderable_content();
    let mut lines = vec![String::new(); term.screen_lines()];
    for cell in content.display_iter {
        // Un carácter ancho (CJK, emoji) ocupa 2 celdas; la otra es un "spacer" sin contenido propio.
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        // Las líneas del historial son negativas; display_offset dice cuánto scrolleamos hacia arriba.
        let row = (cell.point.line.0 + content.display_offset as i32) as usize;
        lines[row].push(cell.c);
    }
    lines
}

/// Traduce una tecla especial a los bytes que espera la shell (convención xterm).
/// El texto normal no pasa por aquí: llega como `Event::Text`.
fn key_bytes(key: egui::Key, mods: egui::Modifiers) -> Option<Vec<u8>> {
    use egui::Key::*;
    let seq: &[u8] = match key {
        Enter => b"\r", // las terminales mandan CR, no LF
        Backspace => b"\x7f",
        Tab => b"\t",
        Escape => b"\x1b",
        ArrowUp => b"\x1b[A",
        ArrowDown => b"\x1b[B",
        ArrowRight => b"\x1b[C",
        ArrowLeft => b"\x1b[D",
        Home => b"\x1b[H",
        End => b"\x1b[F",
        Delete => b"\x1b[3~",
        PageUp => b"\x1b[5~",
        PageDown => b"\x1b[6~",
        // Ctrl+letra = código de control: Ctrl+A = 0x01 ... Ctrl+Z = 0x1A.
        _ if mods.ctrl => {
            let &[c] = key.name().as_bytes() else {
                return None;
            };
            return c.is_ascii_uppercase().then(|| vec![c - b'A' + 1]);
        }
        _ => return None,
    };
    Some(seq.to_vec())
}

/// ConPTY ignora el tamaño en píxeles; solo importan columnas y filas.
fn pty_size((cols, rows): (u16, u16)) -> PtySize {
    PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// ponytail: TermSize vive en `term::test` pero es público y es justo lo que Term::new/resize piden.
fn term_size((cols, rows): (u16, u16)) -> TermSize {
    TermSize::new(cols as usize, rows as usize)
}

/// ¿El cursor está en su fase "encendida"? Alterna cada BLINK desde `since` (el último input).
fn cursor_on(blinking: bool, since: Duration) -> bool {
    !blinking || (since.as_millis() / BLINK.as_millis()) % 2 == 0
}

/// Cuántas celdas de tamaño `cell` caben en `area`. Nunca menos de 1x1: un PTY de 0 columnas falla.
fn grid_size(area: egui::Vec2, cell: egui::Vec2) -> (u16, u16) {
    let fit = |a: f32, c: f32| (a / c).floor().clamp(1.0, u16::MAX as f32) as u16;
    (fit(area.x, cell.x), fit(area.y, cell.y))
}

fn main() -> Result<(), Error> {
    // Tamaño inicial; se corrige en el primer frame, cuando ya sabemos cuánto mide la ventana.
    let grid = (80, 24);
    let shell = spawn_shell(pty_size(grid), &["-NoLogo"])?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Orion")
            .with_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Orion",
        options,
        Box::new(|cc| {
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
            Ok(Box::new(Orion {
                shell,
                rx,
                processor: Processor::new(),
                term,
                grid,
                blink_epoch: Instant::now(),
            }))
        }),
    )?;
    Ok(())
}

struct Orion {
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
}

impl eframe::App for Orion {
    // logic corre antes de cada ui y también con la ventana oculta: así el canal nunca se acumula.
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // try_iter vacía el canal sin bloquear el frame.
        for chunk in self.rx.try_iter() {
            self.processor.advance(&mut self.term, &chunk);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // El teclado va en ui y no en logic: con la ventana oculta, logic vería otra vez los eventos del último frame.
        for event in ui.input(|i| i.events.clone()) {
            let bytes = match event {
                egui::Event::Text(t) => t.into_bytes(),
                // ponytail: sin bracketed paste hasta la issue #13.
                egui::Event::Paste(t) => t.replace('\n', "\r").into_bytes(),
                // egui convierte Ctrl+C/Ctrl+X en Copy/Cut. Sin selección (#13), son señales para la shell.
                egui::Event::Copy => vec![0x03],
                egui::Event::Cut => vec![0x18],
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => match key_bytes(key, modifiers) {
                    Some(b) => b,
                    None => continue,
                },
                _ => continue,
            };
            // Si la shell ya terminó, escribir falla; no hay nada que hacer con el error.
            let _ = self.shell.writer.borrow_mut().write_all(&bytes);
            self.blink_epoch = Instant::now();
        }

        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            // Fuente monoespaciada: todas las letras miden lo mismo, así que 'M' sirve de celda.
            let font = egui::TextStyle::Monospace.resolve(ui.style());
            let cell = ui
                .ctx()
                .fonts_mut(|f| egui::vec2(f.glyph_width(&font, 'M'), f.row_height(&font)));
            let grid = grid_size(ui.available_size(), cell);
            if grid != self.grid {
                self.grid = grid;
                // La grilla y la shell tienen que coincidir: si no, la shell corta líneas donde no debe.
                self.term.resize(term_size(grid));
                let _ = self.shell.master.resize(pty_size(grid));
            }

            // Cada fila se dibuja en una posición exacta: fila * alto de celda.
            // ponytail: sin colores todavía (#10).
            let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
            let color = ui.visuals().text_color();
            let painter = ui.painter();
            for (row, line) in screen_text(&self.term).iter().enumerate() {
                let pos = rect.min + egui::vec2(0.0, row as f32 * cell.y);
                painter.text(
                    pos,
                    egui::Align2::LEFT_TOP,
                    line.trim_end(),
                    font.clone(),
                    color,
                );
            }

            // --- Cursor ---
            let content = self.term.renderable_content();
            let (cursor, offset) = (content.cursor, content.display_offset);
            let focused = ui.input(|i| i.focused);
            let blinking = self.term.cursor_style().blinking && focused;
            let since = self.blink_epoch.elapsed();
            if blinking {
                // egui duerme si nada cambia: le pedimos despertar justo en el próximo cambio de fase.
                let phase = Duration::from_millis((since.as_millis() % BLINK.as_millis()) as u64);
                ui.ctx().request_repaint_after(BLINK - phase);
            }
            // Sin foco, cursor hueco y fijo (como Alacritty/Ghostty): se ve dónde estás, sin distraer.
            let shape = match cursor.shape {
                CursorShape::Hidden => CursorShape::Hidden,
                _ if !focused => CursorShape::HollowBlock,
                shape => shape,
            };
            let row = cursor.point.line.0 + offset as i32;
            // Si scrolleaste hacia arriba (#12), el cursor puede quedar fuera de la vista.
            let in_view = (0..self.grid.1 as i32).contains(&row);
            if shape != CursorShape::Hidden && in_view && cursor_on(blinking, since) {
                let min = rect.min
                    + egui::vec2(cursor.point.column.0 as f32 * cell.x, row as f32 * cell.y);
                let r = egui::Rect::from_min_size(min, cell);
                let thin = 2.0;
                match shape {
                    CursorShape::Block => {
                        // Bloque relleno y, encima, la letra que tapa con el color del fondo.
                        painter.rect_filled(r, 0.0, color);
                        let c = self.term.grid()[cursor.point].c;
                        let bg = ui.visuals().panel_fill;
                        painter.text(r.min, egui::Align2::LEFT_TOP, c, font.clone(), bg);
                    }
                    CursorShape::Beam => {
                        painter.rect_filled(r.with_max_x(r.min.x + thin), 0.0, color);
                    }
                    CursorShape::Underline => {
                        painter.rect_filled(r.with_min_y(r.max.y - thin), 0.0, color);
                    }
                    _ => {
                        let stroke = egui::Stroke::new(1.0, color);
                        painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use std::time::Duration;

    #[test]
    fn shell_output_reaches_reader() {
        let mut shell = spawn_shell(
            pty_size((80, 24)),
            &["-NoProfile", "-Command", "Write-Output orion-ok"],
        )
        .unwrap();
        // Sin Term, respondemos nosotros la pregunta inicial de ConPTY (ESC[6n).
        shell.writer.borrow_mut().write_all(b"\x1b[1;1R").unwrap();
        let rx = spawn_reader(&*shell.master, || {}).unwrap();

        let mut out = String::new();
        while !out.contains("orion-ok") {
            let chunk = rx
                .recv_timeout(Duration::from_secs(15))
                .expect("la shell no imprimió 'orion-ok'");
            out.push_str(&String::from_utf8_lossy(&chunk));
        }
        let _ = shell.child.kill();
    }

    #[test]
    fn term_applies_cursor_moves_and_erase() {
        let mut term = Term::new(Config::default(), &term_size((10, 3)), VoidListener);
        let mut processor: Processor = Processor::new();
        // Escribe, vuelve a (1,1) y sobrescribe; en la fila 2 escribe y borra desde la columna 2.
        processor.advance(
            &mut term,
            b"hola mundo\x1b[1;1HX\x1b[2;1Hxyz\x1b[2;2H\x1b[K\x1b[31m\xC3",
        );
        processor.advance(&mut term, b"\xA1"); // 'á' partida entre dos trozos, en la fila 2
        let lines: Vec<_> = screen_text(&term)
            .iter()
            .map(|l| l.trim_end().to_string())
            .collect();
        assert_eq!(lines, ["Xola mundo", "xá", ""]);
    }

    #[test]
    fn keys_map_to_terminal_bytes() {
        use egui::{Key, Modifiers};
        let none = Modifiers::NONE;
        assert_eq!(key_bytes(Key::Enter, none).unwrap(), b"\r");
        assert_eq!(key_bytes(Key::ArrowUp, none).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes(Key::D, Modifiers::CTRL).unwrap(), [0x04]);
        assert_eq!(key_bytes(Key::A, none), None); // la 'a' llega como Event::Text
        assert_eq!(key_bytes(Key::Num1, Modifiers::CTRL), None);
    }

    #[test]
    fn cursor_blinks_in_phases() {
        let ms = Duration::from_millis;
        assert!(cursor_on(true, ms(0))); // recién tecleado: visible
        assert!(cursor_on(true, ms(529)));
        assert!(!cursor_on(true, ms(530))); // segunda fase: apagado
        assert!(cursor_on(true, ms(1060)));
        assert!(cursor_on(false, ms(530))); // sin parpadeo: siempre visible
    }

    #[test]
    fn grid_size_fits_whole_cells() {
        let cell = egui::vec2(8.0, 16.0);
        assert_eq!(grid_size(egui::vec2(803.0, 390.0), cell), (100, 24));
        assert_eq!(grid_size(egui::vec2(0.0, 0.0), cell), (1, 1));
    }
}
