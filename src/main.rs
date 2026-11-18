use eframe::egui;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Una shell corriendo dentro de un pseudo-terminal (ConPTY en Windows).
#[allow(dead_code)] // ponytail: writer/child se usan desde las issues #5-#7.
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

fn main() -> Result<(), Error> {
    let size = PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    };
    let shell = spawn_shell(size, &["-NoLogo"])?;

    // ponytail: por ahora la salida va a la consola; la issue #5 la manda a la UI.
    let mut reader = shell.master.try_clone_reader()?;
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let mut buf = [0u8; 4096];
        // Flush en cada lectura: ConPTY casi no manda '\n' (posiciona el cursor), y stdout solo hace flush por línea.
        while let Ok(n @ 1..) = reader.read(&mut buf) {
            let mut out = std::io::stdout();
            let _ = out.write_all(&buf[..n]).and_then(|_| out.flush());
        }
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Orion")
            .with_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Orion",
        options,
        Box::new(|_cc| Ok(Box::new(Orion { shell }))),
    )?;
    Ok(())
}

#[allow(dead_code)] // ponytail: se usa desde la issue #5; tenerla aquí mantiene viva la shell.
struct Orion {
    shell: Shell,
}

impl eframe::App for Orion {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            ui.label("Orion");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, sync::mpsc, time::Duration};

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
        let mut reader = shell.master.try_clone_reader().unwrap();

        // Leemos en otro hilo para poder poner un timeout: en ConPTY la lectura no termina sola.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut out = String::new();
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                out.push_str(&String::from_utf8_lossy(&buf[..n]));
                if out.contains("orion-ok") {
                    let _ = tx.send(out);
                    return;
                }
            }
        });

        let out = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("la shell no imprimió 'orion-ok'");
        assert!(out.contains("orion-ok"));
        let _ = shell.child.kill();
    }
}
