use alacritty_terminal::{
    Term,
    event::{Event as TermEvent, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{
        Config, TermMode,
        cell::{Cell, Flags},
        color::Colors,
        test::TermSize,
    },
    vte::ansi::{Color, CursorShape, CursorStyle, NamedColor, Processor, Rgb},
};
use eframe::egui::{
    self, Color32,
    text::{ByteIndex, LayoutJob, LayoutSection, TextFormat},
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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

/// Celdas visibles, una fila por línea de pantalla. El índice dentro de la fila es la columna.
fn visible_rows<T: EventListener>(term: &Term<T>) -> Vec<Vec<&Cell>> {
    let content = term.renderable_content();
    let mut rows = vec![Vec::with_capacity(term.columns()); term.screen_lines()];
    for indexed in content.display_iter {
        // Las líneas del historial son negativas; display_offset dice cuánto scrolleamos hacia arriba.
        let row = (indexed.point.line.0 + content.display_offset as i32) as usize;
        rows[row].push(indexed.cell);
    }
    rows
}

/// Un carácter ancho (CJK, emoji) ocupa 2 celdas; la otra es un "spacer" sin contenido propio.
fn is_spacer(cell: &Cell) -> bool {
    cell.flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
}

// --- Colores ---
// Tema: Tokyo Night. ponytail: fijo hasta tener temas en el config (#25).
const FG: Color32 = Color32::from_rgb(0xc0, 0xca, 0xf5);
const BG: Color32 = Color32::from_rgb(0x1a, 0x1b, 0x26);
/// Los 16 colores ANSI: 0-7 normales, 8-15 brillantes.
const PALETTE: [Color32; 16] = [
    Color32::from_rgb(0x15, 0x16, 0x1e), // negro
    Color32::from_rgb(0xf7, 0x76, 0x8e), // rojo
    Color32::from_rgb(0x9e, 0xce, 0x6a), // verde
    Color32::from_rgb(0xe0, 0xaf, 0x68), // amarillo
    Color32::from_rgb(0x7a, 0xa2, 0xf7), // azul
    Color32::from_rgb(0xbb, 0x9a, 0xf7), // magenta
    Color32::from_rgb(0x7d, 0xcf, 0xff), // cian
    Color32::from_rgb(0xa9, 0xb1, 0xd6), // blanco
    Color32::from_rgb(0x41, 0x48, 0x68), // negro brillante (gris)
    Color32::from_rgb(0xff, 0x89, 0x9d),
    Color32::from_rgb(0x9f, 0xe0, 0x44),
    Color32::from_rgb(0xfa, 0xba, 0x4a),
    Color32::from_rgb(0x8d, 0xb0, 0xff),
    Color32::from_rgb(0xc7, 0xa9, 0xff),
    Color32::from_rgb(0xa4, 0xda, 0xff),
    Color32::from_rgb(0xc0, 0xca, 0xf5),
];

/// "Dim" (SGR 2) = el color a 2/3 de intensidad, igual que Alacritty.
fn dim(c: Color32) -> Color32 {
    let f = |v: u8| (v as f32 * 2.0 / 3.0) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// Color por índice cuando la app no lo redefinió. Numeración de xterm + los extras de Alacritty.
fn default_color(idx: usize) -> Color32 {
    // El cubo 6x6x6 usa estos 6 niveles por canal.
    let level = |v: usize| if v == 0 { 0 } else { (55 + 40 * v) as u8 };
    match idx {
        0..16 => PALETTE[idx],
        16..232 => {
            let i = idx - 16;
            Color32::from_rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        232..256 => {
            let g = (8 + 10 * (idx - 232)) as u8; // 24 grises de 8 a 238
            Color32::from_rgb(g, g, g)
        }
        _ => match idx {
            i if i == NamedColor::Background as usize => BG,
            i if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
                dim(PALETTE[i - NamedColor::DimBlack as usize])
            }
            i if i == NamedColor::DimForeground as usize => dim(FG),
            _ => FG, // Foreground, Cursor, BrightForeground
        },
    }
}

/// Convierte el color de una celda a RGB.
fn resolve(color: Color, colors: &Colors) -> Color32 {
    let idx = match color {
        Color::Spec(Rgb { r, g, b }) => return Color32::from_rgb(r, g, b), // truecolor
        Color::Indexed(i) => i as usize,
        Color::Named(n) => n as usize,
    };
    // Si la app redefinió ese color (OSC 4/10/11), manda eso; si no, nuestra paleta.
    colors[idx].map_or_else(|| default_color(idx), |c| Color32::from_rgb(c.r, c.g, c.b))
}

/// Colores finales (texto, fondo) de una celda, ya aplicados dim, inverso y oculto.
/// Contraste mínimo entre texto y un fondo de color (escala WCAG: 1 = igual, 21 = negro/blanco).
/// 3:1 es el mínimo legible para texto; por debajo, cambiamos el texto a oscuro o claro.
const MIN_CONTRAST: f64 = 3.0;

/// Si `fg` casi no se distingue de `bg`, lo reemplaza por el fondo o el texto del tema, el que contraste más.
/// Ej.: PowerShell pinta carpetas con fondo azul (ESC[44m); el azul de Tokyo Night es claro y el texto claro no se lee.
fn readable(fg: Color32, bg: Color32) -> Color32 {
    let rgb = |c: Color32| Rgb {
        r: c.r(),
        g: c.g(),
        b: c.b(),
    };
    let contrast = |a: Color32| rgb(a).contrast(rgb(bg));
    if contrast(fg) >= MIN_CONTRAST {
        return fg;
    }
    if contrast(BG) > contrast(FG) { BG } else { FG }
}

fn cell_colors(cell: &Cell, colors: &Colors) -> (Color32, Color32) {
    let mut fg = resolve(cell.fg, colors);
    let mut bg = resolve(cell.bg, colors);
    if cell.flags.contains(Flags::DIM) {
        fg = dim(fg);
    }
    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    // Solo sobre fondos de color: el texto tenue sobre el fondo normal (sugerencias de PSReadLine) es intencional.
    if bg != BG {
        fg = readable(fg, bg);
    }
    if cell.flags.contains(Flags::HIDDEN) {
        fg = bg;
    }
    (fg, bg)
}

/// Agrega un carácter al LayoutJob. Si tiene el mismo estilo que el anterior, extiende esa sección:
/// así egui maneja unos pocos tramos por fila en vez de una sección por letra.
fn push_char(job: &mut LayoutJob, c: char, format: TextFormat) {
    let start = ByteIndex(job.text.len());
    job.text.push(c);
    let end = ByteIndex(job.text.len());
    match job.sections.last_mut() {
        Some(s) if s.format == format => s.byte_range.end = end,
        _ => job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: start..end,
            format,
        }),
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

/// ponytail: TermSize vive en `term::test` pero es público y es justo lo que Term::new/resize piden.
fn term_size((cols, rows): (u16, u16)) -> TermSize {
    TermSize::new(cols as usize, rows as usize)
}

/// ¿El cursor está en su fase "encendida"? Alterna cada BLINK desde `since` (el último input).
fn cursor_on(blinking: bool, since: Duration) -> bool {
    !blinking || (since.as_millis() / BLINK.as_millis()) % 2 == 0
}

// --- Configuración ---
// ponytail: se lee al arrancar; la recarga en caliente y el resto de opciones llegan con #24.

/// Contenido inicial de config.toml: se escribe la primera vez para que sea fácil encontrarlo y editarlo.
const DEFAULT_CONFIG: &str = r#"# Configuración de Orion. Los cambios se aplican al reiniciar Orion.

[font]
# Nombre de una fuente instalada (igual que en VS Code), o la ruta a un archivo .ttf/.otf.
# Ejemplos: "Cascadia Mono", "Monocraft", 'C:\fuentes\MiFuente.ttf'
family = "Cascadia Mono"
size = 14.0
"#;

#[derive(serde::Deserialize, Default)]
#[serde(default)] // lo que falte en el archivo toma el valor por defecto
struct Settings {
    font: FontSettings,
}

#[derive(serde::Deserialize)]
#[serde(default)]
struct FontSettings {
    family: String,
    size: f32,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            family: "Cascadia Mono".into(),
            size: 14.0,
        }
    }
}

/// %APPDATA%\orion\config.toml
fn config_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("orion").join("config.toml"))
}

/// Lee la configuración. Si no existe la crea; si tiene errores, avisa y usa los valores por defecto
/// (una coma mal puesta no debería impedir abrir la terminal).
fn load_settings() -> Settings {
    let Some(path) = config_path() else {
        return Settings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
            eprintln!("orion: error en {}:\n{e}", path.display());
            Settings::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let dir = path.parent().expect("la ruta tiene carpeta");
            let _ =
                std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, DEFAULT_CONFIG));
            Settings::default()
        }
        Err(e) => {
            eprintln!("orion: no pude leer {}: {e}", path.display());
            Settings::default()
        }
    }
}

// --- Fuentes ---

/// Familia de fuente para el texto en negrita.
fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

/// ¿Esta entrada del registro de fuentes es de `family`? Some(false) = normal, Some(true) = negrita.
/// Las entradas se ven como "Monocraft (TrueType)", "Cascadia Mono Regular (TrueType)"
/// o, con Windows en español, "Consolas Negrita (TrueType)".
fn face_weight(entry: &str, family: &str) -> Option<bool> {
    let face = entry.rsplit_once(" (").map_or(entry, |(face, _)| face);
    let face = face.to_lowercase();
    let style = face.strip_prefix(&family.to_lowercase())?.trim();
    match style {
        "" | "regular" | "normal" => Some(false),
        "bold" | "negrita" => Some(true),
        _ => None, // otro estilo (cursiva, light…) u otra fuente que empieza igual
    }
}

/// Archivos (normal, negrita si existe) de una fuente: una ruta directa o el nombre de una fuente instalada.
fn find_font(family: &str) -> Option<(PathBuf, Option<PathBuf>)> {
    let lower = family.to_lowercase();
    if lower.ends_with(".ttf") || lower.ends_with(".otf") {
        return Some((family.into(), None));
    }
    use winreg::{RegKey, enums::*};
    const FONTS_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";
    let windows = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    let system_fonts = Path::new(&windows).join("Fonts");
    let (mut regular, mut bold) = (None, None);
    // HKCU: fuentes instaladas solo para tu usuario (guardan la ruta completa).
    // HKLM: fuentes del sistema (guardan solo el nombre del archivo, dentro de C:\Windows\Fonts).
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let Ok(key) = RegKey::predef(root).open_subkey(FONTS_KEY) else {
            continue;
        };
        for (entry, _) in key.enum_values().flatten() {
            let Some(is_bold) = face_weight(&entry, family) else {
                continue;
            };
            let Ok(file) = key.get_value::<String, _>(&entry) else {
                continue;
            };
            // `join` con una ruta absoluta la deja tal cual, así que sirve para ambos casos.
            let slot = if is_bold { &mut bold } else { &mut regular };
            slot.get_or_insert(system_fonts.join(file));
        }
    }
    regular.map(|regular| (regular, bold))
}

/// Fuentes de la terminal: la elegida (normal y negrita) y, detrás, los respaldos.
/// Si a la fuente le falta un carácter, egui lo busca en la siguiente de la lista.
fn font_definitions(files: Option<&(PathBuf, Option<PathBuf>)>) -> egui::FontDefinitions {
    use egui::{FontData, FontFamily, FontTweak, epaint::text::VariationCoords};
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    // Respaldos que ya trae egui (incluyen emoji en blanco y negro).
    let mut fallback = fonts.families[&FontFamily::Monospace].clone();
    // Segoe UI Symbol (viene con Windows): flechas, figuras y símbolos que muchas fuentes no traen.
    if let Some((symbols, _)) = find_font("Segoe UI Symbol")
        && let Ok(bytes) = std::fs::read(symbols)
    {
        let data = Arc::new(FontData::from_owned(bytes));
        fonts.font_data.insert("symbols".into(), data);
        fallback.push("symbols".into());
    }

    let mut mono = fallback.clone();
    let mut bold = fallback;
    if let Some((regular_file, bold_file)) = files
        && let Ok(bytes) = std::fs::read(regular_file)
    {
        // "wght" es el eje de peso de las fuentes variables (400 normal, 700 negrita).
        // En fuentes no variables se ignora.
        let font = |bytes: Vec<u8>, wght: f32| {
            let tweak = FontTweak {
                coords: VariationCoords::new([(b"wght", wght)]),
                ..Default::default()
            };
            Arc::new(FontData::from_owned(bytes).tweak(tweak))
        };
        // Si hay archivo "Bold" aparte, se usa; si no, el mismo archivo con peso 700
        // (una fuente variable como Cascadia engorda; una fija se queda igual).
        let bold_bytes = bold_file.as_ref().and_then(|f| std::fs::read(f).ok());
        let bold_data = font(bold_bytes.unwrap_or_else(|| bytes.clone()), 700.0);
        fonts.font_data.insert("mono".into(), font(bytes, 400.0));
        fonts.font_data.insert("mono-bold".into(), bold_data);
        mono.insert(0, "mono".into());
        bold.insert(0, "mono-bold".into());
    }
    // Sin fuente propia, ambas familias quedan con la de egui (pero existen: egui falla si falta una).
    fonts.families.insert(FontFamily::Monospace, mono);
    fonts.families.insert(bold_family(), bold);
    fonts
}

/// Fondo del texto seleccionado (selección de Tokyo Night).
const SELECTION: Color32 = Color32::from_rgb(0x33, 0x46, 0x7c);

/// Píxel → celda de la grilla, más la mitad de la celda (izq./der.) donde cayó el mouse.
/// `display_offset` traduce la fila en pantalla a la línea real cuando miras el historial.
fn pixel_to_point(
    pos: egui::Pos2,
    origin: egui::Pos2,
    cell: egui::Vec2,
    (cols, rows): (u16, u16),
    display_offset: usize,
) -> (Point, Side) {
    let x = ((pos.x - origin.x) / cell.x).max(0.0);
    let col = (x as usize).min(cols as usize - 1);
    let row = (((pos.y - origin.y) / cell.y).max(0.0) as usize).min(rows as usize - 1);
    // La mitad importa: arrastrar desde la mitad derecha de una letra no la incluye.
    let side = if x.fract() < 0.5 {
        Side::Left
    } else {
        Side::Right
    };
    let line = Line(row as i32 - display_offset as i32);
    (Point::new(line, Column(col)), side)
}

/// Bytes a enviar al pegar. Si la app activó "bracketed paste" (ESC[?2004h), el texto va entre
/// marcadores: así la shell sabe que es un pegado y no ejecuta cada línea como si apretaras Enter.
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        // Sin ESC dentro: un texto pegado con "ESC[201~" podría cerrar el pegado antes de tiempo.
        let clean = text.replace('\x1b', "");
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        // Sin bracketed paste, cada salto de línea es un Enter (CR).
        text.replace('\n', "\r").into_bytes()
    }
}

/// Convierte píxeles de rueda en líneas enteras. Lo que sobra queda en `acc` para el próximo frame:
/// un touchpad manda deltas chicos que por sí solos no llegan a una línea.
fn scroll_lines(acc: &mut f32, delta_px: f32, line_height: f32) -> i32 {
    *acc += delta_px / line_height;
    let lines = acc.trunc();
    *acc -= lines;
    lines as i32
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

    let settings = load_settings();
    let family = &settings.font.family;
    let font_files = find_font(family).or_else(|| {
        eprintln!("orion: no encontré la fuente \"{family}\"; uso Cascadia Mono");
        find_font("Cascadia Mono")
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
        Box::new(|cc| {
            // El Context de egui se puede clonar y usar desde otro hilo para pedir un repaint.
            cc.egui_ctx.set_fonts(font_definitions(font_files.as_ref()));
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
                scroll_acc: 0.0,
                font_size: settings.font.size,
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
    /// Fracción de línea de scroll pendiente (ver `scroll_lines`).
    scroll_acc: f32,
    font_size: f32,
}

impl Orion {
    /// Copia la selección al portapapeles y la quita (señal visual de que se copió).
    /// Devuelve false si no había nada seleccionado.
    fn copy_selection(&mut self, ctx: &egui::Context) -> bool {
        match self.term.selection_to_string() {
            Some(text) if !text.is_empty() => {
                ctx.copy_text(text);
                self.term.selection = None;
                true
            }
            _ => false,
        }
    }
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
        let bracketed = self.term.mode().contains(TermMode::BRACKETED_PASTE);
        // El teclado va en ui y no en logic: con la ventana oculta, logic vería otra vez los eventos del último frame.
        for event in ui.input(|i| i.events.clone()) {
            let bytes = match event {
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
                } => match key_bytes(key, modifiers) {
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

        egui::Frame::central_panel(ui.style())
            .fill(BG)
            .show(ui, |ui| {
                // Fuente monoespaciada: todas las letras miden lo mismo, así que 'M' sirve de celda.
                let font = egui::FontId::new(self.font_size, egui::FontFamily::Monospace);
                let bold_font = egui::FontId::new(self.font_size, bold_family());
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

                // Rueda del mouse: positivo = hacia arriba (historial más viejo).
                let delta = ui.input(|i| i.smooth_scroll_delta.y);
                let lines = scroll_lines(&mut self.scroll_acc, delta, cell.y);
                if lines != 0 {
                    let alt_scroll = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
                    if self.term.mode().contains(alt_scroll) {
                        // Apps de pantalla completa (less, man) no tienen historial: la rueda se vuelve flechas.
                        // ponytail: apps que piden eventos de mouse (vim, btop) quedan para #15.
                        let arrow: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
                        let keys = arrow.repeat(lines.unsigned_abs() as usize);
                        let _ = self.shell.writer.borrow_mut().write_all(&keys);
                    } else {
                        self.term.scroll_display(Scroll::Delta(lines));
                    }
                }

                let (rect, response) =
                    ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

                // --- Selección con el mouse ---
                let offset = self.term.grid().display_offset();
                let to_point = |pos| pixel_to_point(pos, rect.min, cell, grid, offset);
                if let Some(pos) = response.interact_pointer_pos() {
                    let (point, side) = to_point(pos);
                    let start = |ty| Some(Selection::new(ty, point, side));
                    if response.triple_clicked() {
                        self.term.selection = start(SelectionType::Lines); // línea completa
                    } else if response.double_clicked() {
                        self.term.selection = start(SelectionType::Semantic); // palabra
                    } else if response.drag_started() {
                        // egui detecta el arrastre tras unos píxeles: la selección empieza donde apretaste.
                        let pressed = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
                        let (origin, origin_side) = to_point(pressed);
                        let mut selection =
                            Selection::new(SelectionType::Simple, origin, origin_side);
                        selection.update(point, side);
                        self.term.selection = Some(selection);
                    } else if response.dragged() {
                        if let Some(selection) = &mut self.term.selection {
                            selection.update(point, side);
                        }
                    } else if response.clicked() {
                        self.term.selection = None; // clic simple: deseleccionar
                    }
                }

                let painter = ui.painter();
                let colors = self.term.colors();
                let selected = self.term.renderable_content().selection;
                for (row, cells) in visible_rows(&self.term).iter().enumerate() {
                    // Cada fila se dibuja en una posición exacta: fila * alto de celda.
                    let origin = rect.min + egui::vec2(0.0, row as f32 * cell.y);
                    let mut text = LayoutJob::default();

                    for (col, c) in cells.iter().enumerate() {
                        let (mut fg, mut bg) = cell_colors(c, colors);
                        let point = Point::new(Line(row as i32 - offset as i32), Column(col));
                        if selected.is_some_and(|s| s.contains(point)) {
                            bg = SELECTION;
                            fg = readable(fg, bg);
                        }
                        if bg != BG {
                            let width = if c.flags.contains(Flags::WIDE_CHAR) {
                                2.0
                            } else {
                                1.0
                            };
                            let min = origin + egui::vec2(col as f32 * cell.x, 0.0);
                            let size = egui::vec2(width * cell.x, cell.y);
                            painter.rect_filled(egui::Rect::from_min_size(min, size), 0.0, bg);
                        }
                        if is_spacer(c) {
                            continue;
                        }
                        let line = |on: bool| {
                            if on {
                                egui::Stroke::new(1.0, fg)
                            } else {
                                egui::Stroke::NONE
                            }
                        };
                        let format = TextFormat {
                            font_id: if c.flags.contains(Flags::BOLD) {
                                bold_font.clone()
                            } else {
                                font.clone()
                            },
                            color: fg,
                            italics: c.flags.contains(Flags::ITALIC),
                            // ponytail: los 5 tipos de subrayado (doble, ondulado…) se ven como uno simple.
                            underline: line(c.flags.intersects(Flags::ALL_UNDERLINES)),
                            strikethrough: line(c.flags.contains(Flags::STRIKEOUT)),
                            ..Default::default()
                        };
                        push_char(&mut text, c.c, format);
                    }

                    painter.galley(origin, painter.layout_job(text), FG);
                }
                // El cursor usa el color "Cursor" del Term (las apps pueden cambiarlo con OSC 12).
                let color = resolve(Color::Named(NamedColor::Cursor), colors);

                // --- Cursor ---
                let content = self.term.renderable_content();
                let (cursor, offset) = (content.cursor, content.display_offset);
                let focused = ui.input(|i| i.focused);
                let blinking = self.term.cursor_style().blinking && focused;
                let since = self.blink_epoch.elapsed();
                if blinking {
                    // egui duerme si nada cambia: le pedimos despertar justo en el próximo cambio de fase.
                    let phase =
                        Duration::from_millis((since.as_millis() % BLINK.as_millis()) as u64);
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
                            painter.text(r.min, egui::Align2::LEFT_TOP, c, font.clone(), BG);
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
        let lines: Vec<String> = visible_rows(&term)
            .iter()
            .map(|row| {
                let line: String = row.iter().filter(|c| !is_spacer(c)).map(|c| c.c).collect();
                line.trim_end().to_string()
            })
            .collect();
        assert_eq!(lines, ["Xola mundo", "xá", ""]);
    }

    #[test]
    fn palette_256_follows_xterm() {
        let rgb = |r, g, b| Color32::from_rgb(r, g, b);
        assert_eq!(default_color(1), PALETTE[1]);
        assert_eq!(default_color(16), rgb(0, 0, 0)); // inicio del cubo
        assert_eq!(default_color(196), rgb(255, 0, 0)); // rojo puro del cubo
        assert_eq!(default_color(231), rgb(255, 255, 255)); // fin del cubo
        assert_eq!(default_color(232), rgb(8, 8, 8)); // primer gris
        assert_eq!(default_color(255), rgb(238, 238, 238)); // último gris
        assert_eq!(default_color(NamedColor::Background as usize), BG);
        assert_eq!(default_color(NamedColor::DimRed as usize), dim(PALETTE[1]));
    }

    #[test]
    fn cells_get_sgr_colors() {
        let mut term = Term::new(Config::default(), &term_size((10, 1)), VoidListener);
        let mut processor: Processor = Processor::new();
        // X: rojo + inverso. Y: truecolor. Z: índice 256 (verde del cubo).
        processor.advance(
            &mut term,
            b"\x1b[31;7mX\x1b[0m\x1b[38;2;1;2;3mY\x1b[0m\x1b[38;5;46mZ",
        );
        let rows = visible_rows(&term);
        let colors = term.colors();
        assert_eq!(cell_colors(rows[0][0], colors), (BG, PALETTE[1]));
        assert_eq!(
            cell_colors(rows[0][1], colors).0,
            Color32::from_rgb(1, 2, 3)
        );
        assert_eq!(
            cell_colors(rows[0][2], colors).0,
            Color32::from_rgb(0, 255, 0)
        );
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
    fn text_on_colored_background_stays_readable() {
        let mut term = Term::new(Config::default(), &term_size((10, 1)), VoidListener);
        let mut processor: Processor = Processor::new();
        // A: como las carpetas de pwsh (fondo azul + negrita). B: texto tenue sobre el fondo normal.
        processor.advance(&mut term, b"\x1b[44;1mA\x1b[0m\x1b[90mB");
        let rows = visible_rows(&term);
        let colors = term.colors();
        assert_eq!(cell_colors(rows[0][0], colors), (BG, PALETTE[4])); // texto oscuro sobre azul claro
        assert_eq!(cell_colors(rows[0][1], colors).0, PALETTE[8]); // se respeta el gris tenue
        assert_eq!(readable(PALETTE[1], BG), PALETTE[1]); // rojo sobre fondo oscuro ya contrasta
    }

    #[test]
    fn fonts_put_chosen_font_first_with_fallbacks() {
        let files = find_font("Cascadia Mono");
        let fonts = font_definitions(files.as_ref());
        let mono = &fonts.families[&egui::FontFamily::Monospace];
        let bold = &fonts.families[&bold_family()];
        if let Some((regular, _)) = &files {
            assert!(
                regular.exists(),
                "el registro apunta a {regular:?}, que no existe"
            );
            assert_eq!(mono[0], "mono");
            assert_eq!(bold[0], "mono-bold");
        }
        // Normal y negrita comparten los respaldos, después de la fuente principal.
        assert_eq!(mono[1..], bold[1..]);
        assert!(mono.len() > 1);
    }

    #[test]
    fn registry_entries_match_family_and_weight() {
        assert_eq!(
            face_weight("Monocraft (TrueType)", "monocraft"),
            Some(false)
        );
        assert_eq!(
            face_weight("Cascadia Mono Regular (TrueType)", "Cascadia Mono"),
            Some(false)
        );
        assert_eq!(
            face_weight("Consolas Negrita (TrueType)", "Consolas"),
            Some(true)
        );
        assert_eq!(
            face_weight("Consolas Bold (TrueType)", "Consolas"),
            Some(true)
        );
        assert_eq!(face_weight("Consolas Cursiva (TrueType)", "Consolas"), None);
        // "Cascadia Mono" no debe confundirse con "Cascadia Code".
        assert_eq!(
            face_weight("Cascadia Code Regular (TrueType)", "Cascadia Mono"),
            None
        );
        // Una ruta directa no pasa por el registro.
        let direct = find_font(r"C:\x\Mi Fuente.otf").unwrap();
        assert_eq!(direct.0, PathBuf::from(r"C:\x\Mi Fuente.otf"));
    }

    #[test]
    fn config_parses_and_fills_defaults() {
        let s: Settings = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert_eq!(
            (s.font.family.as_str(), s.font.size),
            ("Cascadia Mono", 14.0)
        );
        // Lo que falta toma el valor por defecto; un entero sirve como tamaño.
        let s: Settings = toml::from_str("[font]\nfamily = \"Monocraft\"").unwrap();
        assert_eq!((s.font.family.as_str(), s.font.size), ("Monocraft", 14.0));
        let s: Settings = toml::from_str("[font]\nsize = 16").unwrap();
        assert_eq!(s.font.size, 16.0);
    }

    #[test]
    fn pixels_map_to_cells_and_sides() {
        let (o, cell, grid) = (egui::pos2(0.0, 0.0), egui::vec2(10.0, 20.0), (80, 24));
        let at = |x, y, offset| pixel_to_point(egui::pos2(x, y), o, cell, grid, offset);
        assert_eq!(
            at(22.0, 45.0, 0),
            (Point::new(Line(2), Column(2)), Side::Left)
        );
        assert_eq!(at(27.0, 45.0, 0).1, Side::Right); // mitad derecha de la celda
        assert_eq!(at(22.0, 45.0, 3).0.line, Line(-1)); // con scroll: línea del historial
        // Fuera de la grilla se recorta al borde.
        assert_eq!(at(5000.0, -9.0, 0).0, Point::new(Line(0), Column(79)));
    }

    #[test]
    fn selection_copies_text() {
        let mut term = Term::new(Config::default(), &term_size((20, 2)), VoidListener);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"hola mundo\r\nchao");
        let (p, side) = (Point::new(Line(0), Column(6)), Side::Left);
        term.selection = Some(Selection::new(SelectionType::Semantic, p, side));
        assert_eq!(term.selection_to_string().as_deref(), Some("mundo")); // doble clic
        term.selection = Some(Selection::new(SelectionType::Lines, p, side));
        assert_eq!(term.selection_to_string().as_deref(), Some("hola mundo\n")); // triple clic
    }

    #[test]
    fn paste_respects_bracketed_mode() {
        assert_eq!(paste_bytes("a\nb", false), b"a\rb");
        assert_eq!(paste_bytes("a\nb", true), b"\x1b[200~a\nb\x1b[201~");
        // Un ESC pegado no puede cerrar el bracket antes de tiempo.
        assert_eq!(
            paste_bytes("x\x1b[201~y", true),
            b"\x1b[200~x[201~y\x1b[201~"
        );
    }

    #[test]
    fn scroll_keeps_fractions_between_frames() {
        let mut acc = 0.0;
        assert_eq!(scroll_lines(&mut acc, 50.0, 16.0), 3); // 3.125 líneas: salen 3, sobra 0.125
        assert_eq!(scroll_lines(&mut acc, 10.0, 16.0), 0); // 0.125 + 0.625 = 0.75: todavía no
        assert_eq!(scroll_lines(&mut acc, 4.0, 16.0), 1); // 0.75 + 0.25 = 1: ahora sí
        assert_eq!(scroll_lines(&mut acc, -40.0, 16.0), -2); // hacia abajo: -2.5 → -2
    }

    #[test]
    fn scrolled_history_shows_old_lines() {
        let mut term = Term::new(Config::default(), &term_size((5, 2)), VoidListener);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"uno\r\ndos\r\ntres");
        term.scroll_display(Scroll::Delta(1)); // una línea hacia arriba
        let first: String = visible_rows(&term)[0].iter().map(|c| c.c).collect();
        assert_eq!(first.trim_end(), "uno");
    }

    #[test]
    fn grid_size_fits_whole_cells() {
        let cell = egui::vec2(8.0, 16.0);
        assert_eq!(grid_size(egui::vec2(803.0, 390.0), cell), (100, 24));
        assert_eq!(grid_size(egui::vec2(0.0, 0.0), cell), (1, 1));
    }
}
