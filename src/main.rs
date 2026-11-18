use eframe::egui;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::Read;
use std::sync::mpsc::{self, Receiver};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Una shell corriendo dentro de un pseudo-terminal (ConPTY en Windows).
#[allow(dead_code)] // ponytail: writer/child se usan desde la issue #7.
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

fn main() -> Result<(), Error> {
    let size = PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    };
    let shell = spawn_shell(size, &["-NoLogo"])?;

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
            }))
        }),
    )?;
    Ok(())
}

struct Orion {
    #[allow(dead_code)] // ponytail: se usa desde la issue #7; tenerla aquí mantiene viva la shell.
    shell: Shell,
    rx: Receiver<Vec<u8>>,
    /// Guarda estado entre trozos: una secuencia o un carácter UTF-8 puede llegar partido en dos lecturas.
    parser: vte::Parser,
    /// ponytail: crece sin límite hasta tener scrollback (#12).
    text: PlainText,
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
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
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
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut shell =
            spawn_shell(size, &["-NoProfile", "-Command", "Write-Output orion-ok"]).unwrap();
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
}
