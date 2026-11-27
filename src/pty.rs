//! Pseudo-terminal (ConPTY en Windows): lanzar la shell, leer su salida y responderle.

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::event::EventListener;
use portable_pty::Child;
use portable_pty::CommandBuilder;
use portable_pty::MasterPty;
use portable_pty::PtySize;
use portable_pty::native_pty_system;
use std::cell::{Cell, RefCell};
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

/// Script equivalente para zsh (macOS y Linux). Queda embebido en el binario.
#[cfg(not(windows))]
const ZSH_INTEGRATION: &str = include_str!("shell_integration.zsh");

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
    pub(crate) powershell: bool,
    /// Carga la integración de Orion vía un ZDOTDIR temporal (macOS y Linux).
    #[cfg(not(windows))]
    pub(crate) zsh: bool,
}

/// Las shells que conoce Orion en macOS. La primera instalada es la de siempre.
/// `-l` (login): carga tu perfil, que es donde se arma el PATH (Homebrew, etc.).
#[cfg(not(windows))]
pub(crate) static SHELLS: [ShellKind; 4] = [
    ShellKind {
        name: "zsh",
        program: "zsh",
        args: &["-l"],
        powershell: false,
        zsh: true,
    },
    ShellKind {
        name: "bash",
        program: "bash",
        args: &["-l"],
        powershell: false,
        zsh: false,
    },
    ShellKind {
        name: "fish",
        program: "fish",
        args: &["-l"],
        powershell: false,
        zsh: false,
    },
    ShellKind {
        name: "PowerShell",
        program: "pwsh",
        args: &["-NoLogo"],
        powershell: true,
        zsh: false,
    },
];

/// Si no se encuentra ninguna: la que viene con el sistema.
#[cfg(not(windows))]
const FALLBACK_SHELL: usize = 0; // zsh viene con macOS
#[cfg(windows)]
const FALLBACK_SHELL: usize = 1; // powershell.exe viene con Windows

/// Las shells que conoce Orion en Windows. La primera instalada es la de siempre.
#[cfg(windows)]
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
        installed.first().copied().unwrap_or(FALLBACK_SHELL)
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
        #[cfg(not(windows))]
        if self.zsh {
            return spawn_zsh(size, self.program, &args);
        }
        spawn_program(size, self.program, &args, &[])
    }

    /// Una ruta lista para escribirla en esta shell: entre comillas si hace falta y,
    /// en WSL, como /mnt/c/...
    #[cfg(not(windows))]
    pub(crate) fn quote_path(&self, path: &str) -> String {
        let plain = path
            .chars()
            .all(|c| c.is_alphanumeric() || "/._-+".contains(c));
        if plain {
            path.to_string()
        } else if self.powershell {
            powershell_quote(path)
        } else {
            format!("'{}'", path.replace('\'', r"'\''")) // zsh, bash y fish
        }
    }

    #[cfg(windows)]
    pub(crate) fn quote_path(&self, path: &str) -> String {
        let plain = |p: &str| {
            p.chars()
                .all(|c| c.is_alphanumeric() || r"\/:._-".contains(c))
        };
        if self.program == "wsl.exe" {
            let mut p = path.replace('\\', "/");
            if let [drive, b':', ..] = p.as_bytes() {
                p = format!("/mnt/{}{}", (*drive as char).to_ascii_lowercase(), &p[2..]);
            }
            return if plain(&p) {
                p
            } else {
                format!("'{}'", p.replace('\'', r"'\''"))
            };
        }
        if plain(path) {
            path.to_string()
        } else if self.powershell {
            powershell_quote(path) // con comillas simples, $ no se expande
        } else {
            format!("\"{path}\"") // cmd: una ruta no puede tener comillas dobles
        }
    }
}

/// Texto entre comillas simples para PowerShell. Ojo: PowerShell también toma como comilla simple
/// las tipográficas (‘ ’ ‚ ‛), que los modelos escriben seguido ("l’app"). Sin duplicarlas, una de
/// ellas cerraría el texto y lo que sigue se ejecutaría como comando.
fn powershell_quote(text: &str) -> String {
    let mut quoted = String::from("'");
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            quoted.push(c); // duplicada, PowerShell la lee como una comilla literal
        }
        quoted.push(c);
    }
    quoted.push('\'');
    quoted
}

/// Lanza `program` con `args` en un PTY nuevo.
/// `extra_env`: variables de entorno adicionales (ej. ZDOTDIR para zsh).
///
/// Ojo: ConPTY arranca preguntando la posición del cursor (ESC[6n) y no lanza la shell
/// hasta recibir respuesta. En la app la responde el Term; quien use esto sin Term debe responderla.
pub(crate) fn spawn_program(
    size: PtySize,
    program: &str,
    args: &[impl AsRef<OsStr>],
    extra_env: &[(&str, &OsStr)],
) -> Result<Shell, Error> {
    let pair = native_pty_system().openpty(size)?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    // En macOS, una app abierta desde Finder no trae TERM ni LANG: sin ellos no hay colores
    // y los acentos se ven mal. (En Windows ConPTY no los usa.)
    #[cfg(not(windows))]
    {
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if std::env::var_os("LANG").is_none() {
            cmd.env("LANG", "en_US.UTF-8");
        }
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    // ponytail: el slave se suelta al salir de la función; la shell ya tiene su copia.
    let child = pair.slave.spawn_command(cmd)?;
    let writer = Rc::new(RefCell::new(pair.master.take_writer()?));
    Ok(Shell {
        master: pair.master,
        writer,
        child,
    })
}

/// Escribe los archivos de startup de zsh en `dir`, apuntando al directorio de configuración
/// original del usuario (`zdotdir_src`). Crea wrappers para .zshenv, .zprofile, .zshrc y .zlogin
/// para que el PATH de Homebrew, cargo, etc. lleguen correctamente a la sesión.
#[cfg(not(windows))]
fn zsh_zdotdir(dir: &std::path::Path, zdotdir_src: &std::path::Path) -> Result<(), Error> {
    std::fs::create_dir_all(dir)?;
    let integration = dir.join("orion_integration.zsh");
    std::fs::write(&integration, ZSH_INTEGRATION)?;
    let src = zdotdir_src.display();
    // .zshenv: loaded first, always (login and interactive shells).
    std::fs::write(
        dir.join(".zshenv"),
        format!("[[ -f '{src}/.zshenv' ]] && source '{src}/.zshenv'\n"),
    )?;
    // .zprofile: loaded for login shells, before .zshrc (Homebrew, cargo PATH live here).
    std::fs::write(
        dir.join(".zprofile"),
        format!("[[ -f '{src}/.zprofile' ]] && source '{src}/.zprofile'\n"),
    )?;
    // .zshrc: unset ZDOTDIR first so the user's config finds its own paths, then load integration.
    std::fs::write(
        dir.join(".zshrc"),
        format!(
            "unset ZDOTDIR\n\
             [[ -f '{src}/.zshrc' ]] && source '{src}/.zshrc'\n\
             source '{}'\n",
            integration.display()
        ),
    )?;
    // .zlogin: loaded for login shells, after .zshrc.
    std::fs::write(
        dir.join(".zlogin"),
        format!("[[ -f '{src}/.zlogin' ]] && source '{src}/.zlogin'\n"),
    )?;
    Ok(())
}

/// Lanza zsh con la integración de Orion cargada vía un ZDOTDIR temporal.
#[cfg(not(windows))]
fn spawn_zsh(size: PtySize, program: &str, args: &[String]) -> Result<Shell, Error> {
    let zdotdir_src = std::env::var_os("ZDOTDIR")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let zdotdir = std::env::temp_dir().join(format!("orion_zsh_{}", std::process::id()));
    zsh_zdotdir(&zdotdir, &zdotdir_src)?;
    spawn_program(size, program, args, &[("ZDOTDIR", zdotdir.as_os_str())])
}

/// PowerShell (pwsh, o powershell.exe si no está) con `args`. Para los tests.
#[cfg(all(test, windows))]
pub(crate) fn spawn_shell(size: PtySize, args: &[impl AsRef<OsStr>]) -> Result<Shell, Error> {
    spawn_program(size, SHELLS[usable_shell(0)].program, args, &[])
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
/// - `Bell`: la campana (BEL); la pestaña avisa que algo pide atención.
/// - `ClipboardStore`: un programa copia texto (OSC 52).
pub(crate) struct Replies {
    pub(crate) writer: PtyWriter,
    pub(crate) title: Rc<RefCell<Option<String>>>,
    /// Sonó la campana (BEL): un programa pide atención. La sesión la lee y la apaga.
    pub(crate) bell: Rc<Cell<bool>>,
    /// Texto que un programa pidió copiar (OSC 52). La sesión lo lleva al portapapeles.
    pub(crate) copied: Rc<RefCell<Option<String>>>,
}

impl EventListener for Replies {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::PtyWrite(text) => {
                let _ = self.writer.borrow_mut().write_all(text.as_bytes());
            }
            TermEvent::Title(title) => *self.title.borrow_mut() = Some(title),
            TermEvent::ResetTitle => *self.title.borrow_mut() = None,
            TermEvent::Bell => self.bell.set(true),
            // Leer el portapapeles (ClipboardLoad) no llega nunca: alacritty solo acepta copiar
            // (Osc52::OnlyCopy, su valor por defecto), así un programa no puede espiar lo que copiaste.
            TermEvent::ClipboardStore(_, text) => *self.copied.borrow_mut() = Some(text),
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

    #[cfg(windows)]
    #[test]
    fn dropped_paths_are_quoted_per_shell() {
        let (pwsh, cmd, wsl) = (&SHELLS[0], &SHELLS[2], &SHELLS[3]);
        assert_eq!(pwsh.quote_path(r"C:\src\app.rs"), r"C:\src\app.rs");
        assert_eq!(
            pwsh.quote_path(r"C:\Mis fotos\o'k $x.png"),
            r"'C:\Mis fotos\o''k $x.png'"
        );
        assert_eq!(
            cmd.quote_path(r"C:\Mis fotos\a.png"),
            r#""C:\Mis fotos\a.png""#
        );
        assert_eq!(wsl.quote_path(r"C:\src\app.rs"), "/mnt/c/src/app.rs");
        assert_eq!(wsl.quote_path(r"D:\a b\it's"), r"'/mnt/d/a b/it'\''s'");
    }

    #[cfg(not(windows))]
    #[test]
    fn paths_are_quoted_for_unix_shells() {
        let (zsh, pwsh) = (&SHELLS[0], &SHELLS[3]);
        assert_eq!(zsh.quote_path("/Users/ana/app.rs"), "/Users/ana/app.rs");
        assert_eq!(
            zsh.quote_path("/Users/ana/Mis fotos/it's.png"),
            r"'/Users/ana/Mis fotos/it'\''s.png'"
        );
        assert_eq!(pwsh.quote_path("/a b/it's"), "'/a b/it''s'");
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_shells_run_with_a_terminal_environment() {
        assert_eq!(find_shell("ZSH"), Some(0));
        #[cfg(target_os = "macos")]
        assert!(installed_shells().contains(&0), "zsh viene con macOS");
        let shell =
            spawn_program(pty_size((80, 24)), "sh", &["-c", "echo orion-$TERM"], &[]).unwrap();
        let rx = spawn_reader(&*shell.master, || {}).unwrap();
        wait_for(&rx, &mut String::new(), "orion-xterm-256color");
    }

    #[test]
    fn powershell_quotes_typographic_apostrophes_too() {
        assert_eq!(powershell_quote("l'app"), "'l''app'");
        assert_eq!(powershell_quote("l’app ‘x’"), "'l’’app ‘‘x’’'");
        assert_eq!(powershell_quote("$x; rm"), "'$x; rm'"); // con comillas simples nada se expande
    }

    /// Lo que arma `powershell_quote` vuelve igual de un PowerShell real: ninguna comilla
    /// (recta o tipográfica) corta el texto ni deja ejecutar lo que sigue.
    #[cfg(windows)]
    #[test]
    fn powershell_reads_quoted_text_back_unchanged() {
        let text = "l’app 'x' ‘y’ ‚z‛; Write-Output INYECTADO; # $env:PATH";
        let script = std::env::temp_dir().join(format!("orion-quote-{}.ps1", std::process::id()));
        // Con BOM: así Windows PowerShell también lee el archivo como UTF-8.
        let body = format!(
            "\u{feff}[Console]::OutputEncoding = [Text.Encoding]::UTF8\n[Console]::Out.Write({})",
            powershell_quote(text)
        );
        std::fs::write(&script, body).unwrap();
        let program = SHELLS[usable_shell(0)].program;
        let out = std::process::Command::new(program)
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .env("PSModulePath", "") // más rápido: sin cargar módulos
            .output()
            .unwrap();
        let _ = std::fs::remove_file(&script);
        let printed = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            printed,
            text,
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Lee la salida de la shell hasta que aparezca `needle` (o falla a los 15 s mostrando lo que llegó).
    fn wait_for(rx: &Receiver<Vec<u8>>, out: &mut String, needle: &str) {
        while !out.contains(needle) {
            match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(chunk) => out.push_str(&String::from_utf8_lossy(&chunk)),
                Err(_) => panic!("no llegó {needle:?}; llegó: {out:?}"),
            }
        }
    }

    #[cfg(windows)]
    /// Lanza la shell sin Term: respondemos nosotros la pregunta inicial de ConPTY (ESC[6n).
    fn start(args: &[impl AsRef<OsStr>]) -> (Shell, Receiver<Vec<u8>>) {
        let shell = spawn_shell(pty_size((80, 24)), args).unwrap();
        shell.writer.borrow_mut().write_all(b"\x1b[1;1R").unwrap();
        let rx = spawn_reader(&*shell.master, || {}).unwrap();
        (shell, rx)
    }

    #[cfg(windows)]
    #[test]
    fn shell_output_reaches_reader() {
        let (mut shell, rx) = start(&["-NoProfile", "-Command", "Write-Output orion-ok"]);
        wait_for(&rx, &mut String::new(), "orion-ok");
        let _ = shell.child.kill();
    }

    #[cfg(windows)]
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

    #[cfg(windows)]
    #[test]
    fn shift_enter_adds_a_line_in_powershell() {
        let (shell, rx) = start(&shell_args(&["-NoProfile", "-NoLogo"]));
        let mut out = String::new();
        wait_for(&rx, &mut out, "\x1b]133;B\x07");
        // Lo que manda Orion con Shift+Enter (ESC + Enter) entre dos comandos, y después Enter.
        let typed = b"echo aaa\x1b\recho bbb\r";
        shell.writer.borrow_mut().write_all(typed).unwrap();
        wait_for(&rx, &mut out, "\x1b]133;D;0\x07");
        // ">> " = PSReadLine abrió una segunda línea en vez de ejecutar la primera.
        assert!(
            out.contains(">> "),
            "Shift+Enter no agregó una línea: {out:?}"
        );
        let mut shell = shell;
        let _ = shell.child.kill();
    }

    /// Lanza zsh con solo la integración de Orion (sin perfil de usuario) para los tests.
    #[cfg(all(test, target_os = "macos"))]
    fn start_zsh() -> (Shell, Receiver<Vec<u8>>) {
        let dir = std::env::temp_dir().join(format!("orion_zsh_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let integration = dir.join("orion_integration.zsh");
        std::fs::write(&integration, ZSH_INTEGRATION).unwrap();
        // Minimal .zshrc: just the integration, no user config to keep tests fast and predictable.
        let zshrc = format!("source '{}'\n", integration.display());
        std::fs::write(dir.join(".zshrc"), &zshrc).unwrap();
        let pair = native_pty_system().openpty(pty_size((80, 24))).unwrap();
        let mut cmd = CommandBuilder::new("zsh");
        cmd.args(["-i"]);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("LANG", "en_US.UTF-8");
        cmd.env("ZDOTDIR", &dir);
        cmd.env("HISTFILE", "");
        let child = pair.slave.spawn_command(cmd).unwrap();
        let writer = Rc::new(RefCell::new(pair.master.take_writer().unwrap()));
        let shell = Shell {
            master: pair.master,
            writer,
            child,
        };
        let rx = spawn_reader(&*shell.master, || {}).unwrap();
        (shell, rx)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn zsh_shell_integration_marks_prompt_and_commands() {
        let (mut shell, rx) = start_zsh();
        let mut out = String::new();
        // First prompt: A (starts) and B (ends, starts input).
        wait_for(&rx, &mut out, "\x1b]133;B\x07");
        assert!(out.contains("\x1b]133;A\x07"), "falta A: {out:?}");
        // Tab title: the last component of the path reported in OSC 9;9.
        // We derive it from the shell's own OSC 9;9 output so the test doesn't depend
        // on what directory the shell inherits from the test runner.
        let osc9_start = out.find("\x1b]9;9;").expect("falta OSC 9;9: {out:?}");
        let path_start = osc9_start + "\x1b]9;9;".len();
        let path_end = out[path_start..].find('\x07').unwrap() + path_start;
        let shell_cwd = &out[path_start..path_end];
        let expected_name = std::path::Path::new(shell_cwd)
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new(shell_cwd))
            .to_string_lossy();
        let title = format!("]0;{expected_name}\x07");
        assert!(out.contains(&title), "falta el título {title:?}: {out:?}");

        // A command that fails with code 3: after Enter we get C (output) and D;3 (done).
        out.clear();
        shell.writer.borrow_mut().write_all(b"(exit 3)\r").unwrap();
        wait_for(&rx, &mut out, "\x1b]133;D;3\x07");
        let c = out.find("\x1b]133;C\x07").expect("falta C");
        assert!(
            c < out.find("\x1b]133;D;3").unwrap(),
            "C debe llegar antes que D"
        );
        let _ = shell.child.kill();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn zsh_shift_enter_adds_a_line() {
        let (shell, rx) = start_zsh();
        let mut out = String::new();
        wait_for(&rx, &mut out, "\x1b]133;B\x07");
        out.clear();
        // ESC+Enter between two commands, then Enter: both run as a single multi-line command.
        let typed = b"echo aaa\x1b\recho bbb\r";
        shell.writer.borrow_mut().write_all(typed).unwrap();
        wait_for(&rx, &mut out, "\x1b]133;D;0\x07");
        assert!(out.contains("aaa"), "falta 'aaa' en la salida: {out:?}");
        assert!(out.contains("bbb"), "falta 'bbb' en la salida: {out:?}");
        // A single C means both commands executed in one go, not two separate runs.
        let c_count = out.matches("\x1b]133;C\x07").count();
        assert_eq!(c_count, 1, "se esperaba una sola ejecución (un C): {out:?}");
        let mut shell = shell;
        let _ = shell.child.kill();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn zsh_profile_variables_reach_the_shell() {
        // Create a fake user config dir with a .zprofile that exports a known variable.
        // This exercises the full ZDOTDIR wrapper chain (.zprofile → .zshrc → integration).
        let fake_src =
            std::env::temp_dir().join(format!("orion_zsh_profile_src_{}", std::process::id()));
        std::fs::create_dir_all(&fake_src).unwrap();
        std::fs::write(
            fake_src.join(".zprofile"),
            "export ORION_TEST_PROFILE=from_zprofile\n",
        )
        .unwrap();

        // Build the ZDOTDIR structure the same way Orion does in production.
        let zdotdir =
            std::env::temp_dir().join(format!("orion_zsh_profile_zdotdir_{}", std::process::id()));
        super::zsh_zdotdir(&zdotdir, &fake_src).unwrap();

        // Spawn with -l (login) + PTY, exactly like ShellKind { args: &["-l"] }.
        let pair = native_pty_system().openpty(pty_size((80, 24))).unwrap();
        let mut cmd = CommandBuilder::new("zsh");
        cmd.args(["-l"]);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("LANG", "en_US.UTF-8");
        cmd.env("ZDOTDIR", &zdotdir);
        cmd.env("HISTFILE", "");
        let child = pair.slave.spawn_command(cmd).unwrap();
        let writer = Rc::new(RefCell::new(pair.master.take_writer().unwrap()));
        let shell = Shell {
            master: pair.master,
            writer,
            child,
        };
        let rx = spawn_reader(&*shell.master, || {}).unwrap();

        let mut out = String::new();
        wait_for(&rx, &mut out, "\x1b]133;B\x07");
        out.clear();
        shell
            .writer
            .borrow_mut()
            .write_all(b"echo \"profile:$ORION_TEST_PROFILE\"\r")
            .unwrap();
        wait_for(&rx, &mut out, "profile:from_zprofile");
        let mut shell = shell;
        let _ = shell.child.kill();
    }

    #[test]
    fn programs_can_copy_with_osc52() {
        use alacritty_terminal::Term;
        use alacritty_terminal::term::Config;
        use alacritty_terminal::vte::ansi::Processor;
        let copied = Rc::new(RefCell::new(None));
        let writer: Box<dyn Write + Send> = Box::new(Vec::new());
        let replies = Replies {
            writer: Rc::new(RefCell::new(writer)),
            title: Rc::new(RefCell::new(None)),
            bell: Rc::new(Cell::new(false)),
            copied: copied.clone(),
        };
        let mut term = Term::new(Config::default(), &crate::grid::term_size((10, 2)), replies);
        // OSC 52: "c" = portapapeles, y el texto en base64 ("aG9sYQ==" = "hola").
        Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new()
            .advance(&mut term, b"\x1b]52;c;aG9sYQ==\x07");
        assert_eq!(copied.borrow().as_deref(), Some("hola"));
    }

    #[cfg(windows)]
    #[test]
    fn other_shells_run_too() {
        // cmd viene con Windows: siempre está.
        let cmd = find_shell("CMD").unwrap();
        assert!(installed_shells().contains(&cmd));
        assert_eq!(find_shell("nada"), None);
        let shell = spawn_program(
            pty_size((80, 24)),
            "cmd.exe",
            &["/c", "echo orion-cmd"],
            &[],
        )
        .unwrap();
        shell.writer.borrow_mut().write_all(b"\x1b[1;1R").unwrap();
        let rx = spawn_reader(&*shell.master, || {}).unwrap();
        wait_for(&rx, &mut String::new(), "orion-cmd");
    }

    #[cfg(windows)]
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
