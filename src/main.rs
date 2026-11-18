use eframe::egui;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Una shell corriendo dentro de un pseudo-terminal (ConPTY en Windows).
#[allow(dead_code)] // ponytail: child solo se usa en tests hasta manejar el cierre de la shell.
struct Shell {
    /// Lado "terminal" del PTY: leemos lo que la shell imprime y escribimos lo que el usuario teclea.
    master: Box<dyn MasterPty + Send>,
    /// Input hacia la shell (teclado). Solo se puede tomar una vez del master.
    writer: Box<dyn std::io::Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

/// Lanza pwsh (o powershell.exe si no está) con `args` extra.
fn spawn_shell(size: PtySize, args: &[&str]) -> Result<Shell, Error> {
    let pair = native_pty_system().openpty(size)?;
    let mut last_err = None;
    for program in ["pwsh.exe", "powershell.exe"] {
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        match pair.slave.spawn_command(cmd) {
            // ponytail: el slave se suelta al salir de la función; la shell ya tiene su copia.
            Ok(child) => {
                let mut writer = pair.master.take_writer()?;
                // ConPTY arranca preguntando la posición del cursor (ESC[6n) y no lanza la shell
                // hasta recibir respuesta. ponytail: respondemos 1;1 a ciegas; alacritty_terminal (#9) lo hará bien.
                writer.write_all(b"\x1b[1;1R")?;
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

/// Recibe los eventos del parser VT y se queda solo con el texto imprimible y los saltos de línea.
/// Ignora colores, movimientos de cursor, títulos de ventana, etc.
/// ponytail: descarta el posicionamiento del cursor; la grilla real llega con alacritty_terminal (#9).
#[derive(Default)]
struct PlainText(String);

impl vte::Perform for PlainText {
    fn print(&mut self, c: char) {
        self.0.push(c);
    }

    // Caracteres de control (C0): solo nos interesa '\n'; '\r', BEL, etc. se descartan.
    fn execute(&mut self, byte: u8) {
        if byte == b'\n' {
            self.0.push('\n');
        }
    }
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
            Ok(Box::new(Orion {
                shell,
                rx,
                parser: vte::Parser::new(),
                text: PlainText::default(),
                grid,
            }))
        }),
    )?;
    Ok(())
}

struct Orion {
    shell: Shell,
    rx: Receiver<Vec<u8>>,
    /// Guarda estado entre trozos: una secuencia o un carácter UTF-8 puede llegar partido en dos lecturas.
    parser: vte::Parser,
    /// ponytail: crece sin límite hasta tener scrollback (#12).
    text: PlainText,
    /// Columnas y filas que la shell cree tener. Se compara cada frame para avisarle solo si cambia.
    grid: (u16, u16),
}

impl eframe::App for Orion {
    // logic corre antes de cada ui y también con la ventana oculta: así el canal nunca se acumula.
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // try_iter vacía el canal sin bloquear el frame.
        for chunk in self.rx.try_iter() {
            self.parser.advance(&mut self.text, &chunk);
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
            let _ = self.shell.writer.write_all(&bytes);
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
                // ConPTY le avisa a la shell, que reacomoda su salida (por ejemplo, dónde corta las líneas).
                let _ = self.shell.master.resize(pty_size(grid));
            }

            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(&self.text.0).monospace());
                });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn shell_output_reaches_reader() {
        let mut shell = spawn_shell(
            pty_size((80, 24)),
            &["-NoProfile", "-Command", "Write-Output orion-ok"],
        )
        .unwrap();
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
    fn plain_text_strips_escapes() {
        let mut parser = vte::Parser::new();
        let mut text = PlainText::default();
        // Color, título de ventana (OSC), \r\n, y una 'á' (0xC3 0xA1) partida entre dos trozos.
        parser.advance(&mut text, b"\x1b[31mhola\x1b[0m\r\n\x1b]0;titulo\x07m\xC3");
        parser.advance(&mut text, b"\xA1s\x1b[?25h");
        assert_eq!(text.0, "hola\nmás");
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
    fn grid_size_fits_whole_cells() {
        let cell = egui::vec2(8.0, 16.0);
        assert_eq!(grid_size(egui::vec2(803.0, 390.0), cell), (100, 24));
        assert_eq!(grid_size(egui::vec2(0.0, 0.0), cell), (1, 1));
    }
}
