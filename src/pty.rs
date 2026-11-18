//! Pseudo-terminal (ConPTY en Windows): lanzar la shell, leer su salida y responderle.

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::event::EventListener;
use portable_pty::Child;
use portable_pty::CommandBuilder;
use portable_pty::MasterPty;
use portable_pty::PtySize;
use portable_pty::native_pty_system;
use std::cell::RefCell;
use std::ffi::OsStr;
use std::io::Read;
use std::io::Write;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;

pub(crate) type Error = Box<dyn std::error::Error + Send + Sync>;

/// Input hacia la shell. Compartido: escriben el teclado (UI) y las respuestas automáticas del Term.
/// ponytail: Rc<RefCell> porque todo corre en el hilo de la UI; pasaría a Arc<Mutex> si otro hilo escribiera.
pub(crate) type PtyWriter = Rc<RefCell<Box<dyn Write + Send>>>;

/// Una shell corriendo dentro de un pseudo-terminal (ConPTY en Windows).
pub(crate) struct Shell {
    /// Lado "terminal" del PTY: leemos lo que la shell imprime y le avisamos cambios de tamaño.
    pub(crate) master: Box<dyn MasterPty + Send>,
    pub(crate) writer: PtyWriter,
    pub(crate) child: Box<dyn Child + Send + Sync>,
}

/// Script que marca prompts y comandos con OSC 133 (ver el archivo). Queda dentro del .exe.
const SHELL_INTEGRATION: &str = include_str!("shell_integration.ps1");

/// Argumentos para PowerShell: los de `extra`, más cargar la integración y quedarse abierta.
/// `-EncodedCommand` recibe el script en base64 de UTF-16: así no hay que escapar comillas ni saltos de línea.
pub(crate) fn shell_args(extra: &[&str]) -> Vec<String> {
    use base64::Engine;
    let utf16: Vec<u8> = SHELL_INTEGRATION
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let script = base64::engine::general_purpose::STANDARD.encode(utf16);
    let mut args: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
    args.extend(["-NoExit".into(), "-EncodedCommand".into(), script]);
    args
}

/// Una shell que se puede elegir para una pestaña.
pub(crate) struct ShellKind {
    pub(crate) name: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    /// Carga la integración de Orion (bloques, título de la pestaña, sugerencias).
    /// ponytail: solo PowerShell; cmd y WSL andan como terminal normal, sin bloques.
    powershell: bool,
}

/// Las shells que conoce Orion. La primera instalada es la de siempre.
pub(crate) static SHELLS: [ShellKind; 4] = [
    ShellKind {
        name: "PowerShell",
        program: "pwsh.exe",
        args: &["-NoLogo"],
        powershell: true,
    },
    ShellKind {
        name: "Windows PowerShell",
        program: "powershell.exe",
        args: &["-NoLogo"],
        powershell: true,
    },
    ShellKind {
        name: "cmd",
        program: "cmd.exe",
        args: &[],
        powershell: false,
    },
    ShellKind {
        name: "WSL",
        program: "wsl.exe",
        args: &["--cd", "~"], // tu carpeta de Linux, no /mnt/c/Users/...
        powershell: false,
    },
];

/// Índice de la shell con ese nombre (sin importar mayúsculas).
pub(crate) fn find_shell(name: &str) -> Option<usize> {
    SHELLS
        .iter()
        .position(|s| s.name.eq_ignore_ascii_case(name.trim()))
}

/// Índices de las shells instaladas (su programa está en el PATH). Se calcula una vez.
pub(crate) fn installed_shells() -> &'static [usize] {
    static INSTALLED: OnceLock<Vec<usize>> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let dirs: Vec<_> = std::env::split_paths(&path).collect();
        let found = |program: &str| dirs.iter().any(|d| d.join(program).is_file());
        (0..SHELLS.len())
            .filter(|&i| found(SHELLS[i].program))
            .collect()
    })
}

/// La shell `index` si está instalada; si no, la primera que sí (ej. sin pwsh, Windows PowerShell).
pub(crate) fn usable_shell(index: usize) -> usize {
    let installed = installed_shells();
    if installed.contains(&index) {
        index
    } else {
        installed.first().copied().unwrap_or(1) // powershell.exe viene con Windows
    }
}

impl ShellKind {
    /// Lanza esta shell en un PTY nuevo.
    pub(crate) fn spawn(&self, size: PtySize) -> Result<Shell, Error> {
        let args: Vec<String> = if self.powershell {
            shell_args(self.args)
        } else {
            self.args.iter().map(|s| s.to_string()).collect()
        };
        spawn_program(size, self.program, &args)
    }
}

/// Lanza `program` con `args` en un PTY nuevo.
///
/// Ojo: ConPTY arranca preguntando la posición del cursor (ESC[6n) y no lanza la shell
/// hasta recibir respuesta. En la app la responde el Term; quien use esto sin Term debe responderla.
pub(crate) fn spawn_program(
    size: PtySize,
    program: &str,
    args: &[impl AsRef<OsStr>],
) -> Result<Shell, Error> {
    let pair = native_pty_system().openpty(size)?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    // ponytail: el slave se suelta al salir de la función; la shell ya tiene su copia.
    let child = pair.slave.spawn_command(cmd)?;
    let writer = Rc::new(RefCell::new(pair.master.take_writer()?));
    Ok(Shell {
        master: pair.master,
        writer,
        child,
    })
}

/// PowerShell (pwsh, o powershell.exe si no está) con `args`. Para los tests.
#[cfg(test)]
pub(crate) fn spawn_shell(size: PtySize, args: &[impl AsRef<OsStr>]) -> Result<Shell, Error> {
    spawn_program(size, SHELLS[usable_shell(0)].program, args)
}

/// Lee el PTY en un hilo propio y manda cada trozo de bytes por un canal.
/// `read` bloquea, por eso no puede ir en el hilo de la UI: congelaría la ventana.
/// `notify` se llama tras cada envío; la UI lo usa para despertarse y repintar.
pub(crate) fn spawn_reader(
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

/// Recibe los avisos del Term. Nos importan dos:
/// - `PtyWrite`: respuestas que la terminal debe mandarle a la shell, como la posición del cursor
///   cuando la shell la pregunta (ESC[6n).
/// - `Title`: el título que pone la shell (OSC 0); lo usa la pestaña.
pub(crate) struct Replies {
    pub(crate) writer: PtyWriter,
    pub(crate) title: Rc<RefCell<Option<String>>>,
}

impl EventListener for Replies {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::PtyWrite(text) => {
                let _ = self.writer.borrow_mut().write_all(text.as_bytes());
            }
            TermEvent::Title(title) => *self.title.borrow_mut() = Some(title),
            TermEvent::ResetTitle => *self.title.borrow_mut() = None,
            _ => {}
        }
    }
}

/// ConPTY ignora el tamaño en píxeles; solo importan columnas y filas.
pub(crate) fn pty_size((cols, rows): (u16, u16)) -> PtySize {
    PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Lee la salida de la shell hasta que aparezca `needle` (o falla a los 15 s mostrando lo que llegó).
    fn wait_for(rx: &Receiver<Vec<u8>>, out: &mut String, needle: &str) {
        while !out.contains(needle) {
            match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(chunk) => out.push_str(&String::from_utf8_lossy(&chunk)),
                Err(_) => panic!("no llegó {needle:?}; llegó: {out:?}"),
            }
        }
    }

    /// Lanza la shell sin Term: respondemos nosotros la pregunta inicial de ConPTY (ESC[6n).
    fn start(args: &[impl AsRef<OsStr>]) -> (Shell, Receiver<Vec<u8>>) {
        let shell = spawn_shell(pty_size((80, 24)), args).unwrap();
        shell.writer.borrow_mut().write_all(b"\x1b[1;1R").unwrap();
        let rx = spawn_reader(&*shell.master, || {}).unwrap();
        (shell, rx)
    }

    #[test]
    fn shell_output_reaches_reader() {
        let (mut shell, rx) = start(&["-NoProfile", "-Command", "Write-Output orion-ok"]);
        wait_for(&rx, &mut String::new(), "orion-ok");
        let _ = shell.child.kill();
    }

    #[test]
    fn shell_integration_marks_prompt_and_commands() {
        let (mut shell, rx) = start(&shell_args(&["-NoProfile", "-NoLogo"]));
        let mut out = String::new();
        // Primer prompt: A (empieza) y B (termina, empieza lo que escribes).
        wait_for(&rx, &mut out, "\x1b]133;B\x07");
        assert!(out.contains("\x1b]133;A\x07"), "falta A: {out:?}");
        // Título de la pestaña: la carpeta actual. La shell arranca en tu carpeta de usuario (ej. "benja").
        let home = std::env::var("USERPROFILE").unwrap();
        let user = std::path::Path::new(&home).file_name().unwrap();
        let title = format!("]0;{}\x07", user.to_string_lossy());
        assert!(out.contains(&title), "falta el título {title:?}: {out:?}");

        // Un comando que falla con código 3: después del Enter llegan C (salida) y D;3 (terminó).
        out.clear();
        let command = b"cmd /c exit 3\r";
        shell.writer.borrow_mut().write_all(command).unwrap();
        wait_for(&rx, &mut out, "\x1b]133;D;3\x07");
        let c = out.find("\x1b]133;C\x07").expect("falta C");
        assert!(
            c < out.find("\x1b]133;D;3").unwrap(),
            "C debe llegar antes que D"
        );
        let _ = shell.child.kill();
    }

    #[test]
    fn other_shells_run_too() {
        // cmd viene con Windows: siempre está.
        let cmd = find_shell("CMD").unwrap();
        assert!(installed_shells().contains(&cmd));
        assert_eq!(find_shell("nada"), None);
        let shell = spawn_program(pty_size((80, 24)), "cmd.exe", &["/c", "echo orion-cmd"]);
        let shell = shell.unwrap();
        shell.writer.borrow_mut().write_all(b"\x1b[1;1R").unwrap();
        let rx = spawn_reader(&*shell.master, || {}).unwrap();
        wait_for(&rx, &mut String::new(), "orion-cmd");
    }

    #[test]
    fn shell_integration_turns_on_suggestions() {
        let (mut shell, rx) = start(&shell_args(&["-NoProfile", "-NoLogo"]));
        let mut out = String::new();
        wait_for(&rx, &mut out, "\x1b]133;B\x07");
        // Windows PowerShell 5.1 trae PSReadLine 2.0, sin sugerencias: ahí no hay nada que probar.
        let command = b"if ((Get-Module PSReadLine).Version -ge '2.1') { 'pred:' + (Get-PSReadLineOption).PredictionSource } else { 'pred:History' }\r";
        shell.writer.borrow_mut().write_all(command).unwrap();
        // "History" o "HistoryAndPlugin". El eco del comando no contiene "pred:H" (ahí va "'pred:'").
        wait_for(&rx, &mut out, "pred:History");
        let _ = shell.child.kill();
    }
}
