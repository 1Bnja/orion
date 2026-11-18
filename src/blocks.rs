//! Bloques de comandos (estilo Warp): detectar las marcas OSC 133 que manda la shell
//! y armar, para cada comando, qué se escribió, dónde está su salida, su código y su duración.
//!
//! Las marcas las pone `shell_integration.ps1`:
//!   A = empieza el prompt, B = empieza lo que escribes, C = empieza la salida, D;n = terminó (código n).

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use std::time::{Duration, Instant};

/// Una marca OSC 133.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Mark {
    /// A: empieza el prompt.
    PromptStart,
    /// B: termina el prompt; lo que sigue es el comando que escribe el usuario.
    CommandStart,
    /// C: el usuario apretó Enter; lo que sigue es la salida.
    OutputStart,
    /// D: terminó el comando. `None` = Enter sin comando (o la shell no informó el código).
    Finished(Option<i32>),
}

/// Lo que sale del escáner, en el mismo orden en que llegó.
#[derive(Debug, PartialEq)]
pub(crate) enum Piece {
    /// Bytes normales: van al parser VT como siempre.
    Data(Vec<u8>),
    Mark(Mark),
}

const PREFIX: &[u8] = b"\x1b]133;";
/// Una marca más larga que esto no es una marca de verdad: se deja pasar como texto.
const MAX_MARK: usize = 256;

/// Separa las marcas OSC 133 del resto de la salida de la shell.
///
/// Tiene estado porque una marca puede llegar partida entre dos lecturas del PTY
/// (por ejemplo `ESC]13` en una y `3;D;0 BEL` en la siguiente).
#[derive(Default)]
pub(crate) struct MarkScanner {
    /// Bytes del final de la lectura anterior que podrían ser el comienzo de una marca.
    pending: Vec<u8>,
}

impl MarkScanner {
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<Piece> {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(chunk);
        let mut pieces = Vec::new();
        let mut plain = 0; // desde dónde hay bytes normales sin entregar
        let mut i = 0;
        while let Some(esc) = data[i..].iter().position(|&b| b == 0x1b).map(|p| p + i) {
            let rest = &data[esc..];
            if !rest.starts_with(PREFIX) {
                if PREFIX.starts_with(rest) {
                    // La lectura terminó a mitad del prefijo: esperamos la siguiente.
                    self.pending = rest.to_vec();
                    data.truncate(esc);
                    break;
                }
                i = esc + 1;
                continue;
            }
            // Terminador: BEL, o ST (ESC \).
            let body = &rest[PREFIX.len()..];
            let end = body
                .iter()
                .enumerate()
                .find(|&(j, &b)| b == 0x07 || (b == 0x1b && body.get(j + 1) == Some(&b'\\')));
            match end {
                Some((j, &terminator)) => {
                    if esc > plain {
                        pieces.push(Piece::Data(data[plain..esc].to_vec()));
                    }
                    if let Some(mark) = parse_mark(&body[..j]) {
                        pieces.push(Piece::Mark(mark));
                    }
                    let len = PREFIX.len() + j + if terminator == 0x07 { 1 } else { 2 };
                    i = esc + len;
                    plain = i;
                }
                None if rest.len() < MAX_MARK => {
                    // Marca incompleta: el resto llegará en la próxima lectura.
                    self.pending = rest.to_vec();
                    data.truncate(esc);
                    break;
                }
                None => i = esc + 1, // demasiado larga: no es una marca
            }
        }
        if data.len() > plain {
            pieces.push(Piece::Data(data[plain..].to_vec()));
        }
        pieces
    }
}

/// "A", "B", "C", "D" o "D;3" → la marca. El resto de parámetros (si los hay) se ignora.
fn parse_mark(body: &[u8]) -> Option<Mark> {
    let text = std::str::from_utf8(body).ok()?;
    let mut parts = text.split(';');
    Some(match parts.next()? {
        "A" => Mark::PromptStart,
        "B" => Mark::CommandStart,
        "C" => Mark::OutputStart,
        "D" => Mark::Finished(parts.next().and_then(|code| code.parse().ok())),
        _ => return None,
    })
}

/// Un comando con su salida. Las posiciones son *líneas lógicas* (ver `logical_line`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Block {
    /// Línea donde empieza el prompt.
    pub(crate) prompt: usize,
    /// Línea donde empieza la salida (None mientras escribes el comando).
    pub(crate) output: Option<usize>,
    /// Línea donde terminó (exclusiva: ahí empieza el prompt siguiente). None = sigue corriendo.
    pub(crate) end: Option<usize>,
    /// Lo que escribiste.
    pub(crate) command: String,
    pub(crate) exit_code: Option<i32>,
    /// Desde el Enter hasta que terminó.
    pub(crate) duration: Option<Duration>,
    started: Option<Instant>,
    /// Dónde empezó el comando (marca B): fila absoluta (contando el historial) y columna.
    command_start: Option<(usize, usize)>,
    /// Colapsado: se ve solo el prompt con el comando, la salida queda oculta.
    pub(crate) collapsed: bool,
}

/// En qué está un bloque.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Status {
    /// Estás escribiendo el comando (todavía no apretaste Enter).
    Editing,
    /// El comando está corriendo, hace este tiempo.
    Running(Duration),
    /// Terminó, con su código (si la shell lo informó) y lo que tardó.
    Done(Option<i32>, Duration),
}

impl Block {
    pub(crate) fn status(&self) -> Status {
        match (self.output, self.end) {
            (None, _) => Status::Editing,
            (Some(_), None) => {
                Status::Running(self.started.map_or_else(Duration::default, |t| t.elapsed()))
            }
            (Some(_), Some(_)) => Status::Done(self.exit_code, self.duration.unwrap_or_default()),
        }
    }

    /// Líneas lógicas que ocupa, `[desde, hasta)`. Un bloque abierto llega hasta la línea del cursor.
    pub(crate) fn span(&self, cursor_line: usize) -> (usize, usize) {
        (self.prompt, self.end.unwrap_or(cursor_line + 1))
    }

    /// Cuántas líneas lógicas tiene la salida (0 si todavía no hay o no terminó).
    pub(crate) fn output_lines(&self) -> usize {
        match (self.output, self.end) {
            (Some(start), Some(end)) => end.saturating_sub(start),
            _ => 0,
        }
    }

    /// El texto de la salida, tal como se ve (sin colores). None si el bloque no terminó
    /// o si su salida ya no está en el historial.
    pub(crate) fn output_text<T>(&self, term: &Term<T>) -> Option<String> {
        let (start, end) = (self.output?, self.end?);
        if start >= end {
            return Some(String::new()); // un comando sin salida
        }
        let first = grid_line_of(term, start)?;
        // `end` es la primera línea que NO es salida: la salida termina en la fila de arriba.
        let last = grid_line_of(term, end)? - 1;
        let text = term.bounds_to_string(
            Point::new(first, Column(0)),
            Point::new(last, term.grid().last_column()),
        );
        Some(text.trim_end().to_string())
    }
}

/// "120 ms", "2.3 s", "1 min 5 s": corto y fácil de leer de un vistazo.
pub(crate) fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", d.as_secs_f32())
    } else {
        format!("{} min {} s", ms / 60_000, ms / 1000 % 60)
    }
}

/// Primera y última fila visible (inclusive) cuyas líneas lógicas están en `[start, end)`.
/// `lines[fila]` = línea lógica de esa fila (ver `visible_logical_lines`).
pub(crate) fn visible_rows_of(
    lines: &[usize],
    (start, end): (usize, usize),
) -> Option<(usize, usize)> {
    let inside = |l: &usize| (start..end).contains(l);
    let first = lines.iter().position(inside)?;
    let last = lines.iter().rposition(inside)?;
    Some((first, last))
}

/// Línea lógica de cada fila visible (fila 0 = arriba de la pantalla, teniendo en cuenta el scroll).
pub(crate) fn visible_logical_lines<T>(term: &Term<T>) -> Vec<usize> {
    let grid = term.grid();
    let last = grid.last_column();
    let top = Line(-(grid.display_offset() as i32));
    let mut logical = logical_line(term, top);
    (0..grid.screen_lines() as i32)
        .map(|row| {
            let here = logical;
            // La línea lógica avanza cuando una fila no sigue en la de abajo.
            if !grid[top + row][last].flags.contains(Flags::WRAPLINE) {
                logical += 1;
            }
            here
        })
        .collect()
}

/// Los bloques de la sesión, del más viejo al más nuevo.
#[derive(Default)]
pub(crate) struct Blocks {
    pub(crate) list: Vec<Block>,
    /// Para notar un `cls` (ESC[3J borra el historial): las posiciones viejas dejan de valer.
    history: usize,
}

impl Blocks {
    /// Aplica una marca. Hay que llamarla justo cuando llega, para que el cursor esté donde la shell la puso.
    pub(crate) fn on_mark<T: EventListener>(&mut self, mark: Mark, term: &Term<T>) {
        let cursor = term.grid().cursor.point;
        let line = logical_line(term, cursor.line);
        match mark {
            Mark::PromptStart => self.list.push(Block {
                prompt: line,
                output: None,
                end: None,
                command: String::new(),
                exit_code: None,
                duration: None,
                started: None,
                command_start: None,
                collapsed: false,
            }),
            Mark::CommandStart => {
                if let Some(block) = self.current() {
                    block.command_start = Some((absolute_row(term, cursor.line), cursor.column.0));
                }
            }
            Mark::OutputStart => {
                if let Some(block) = self.current() {
                    block.output = Some(line);
                    block.started = Some(Instant::now());
                    // Lo escrito va desde la marca B hasta justo antes del cursor actual.
                    if let Some((row, col)) = block.command_start {
                        let start = Point::new(grid_line(term, row), Column(col));
                        if let Some(end) = before(term, cursor) {
                            block.command = term.bounds_to_string(start, end).trim().to_string();
                        }
                    }
                }
            }
            Mark::Finished(code) => {
                let Some(block) = self.current() else { return };
                if block.output.is_none() {
                    // Enter sin comando, o Ctrl+C mientras escribías: no hay bloque que mostrar.
                    self.list.pop();
                    return;
                }
                block.end = Some(line);
                block.exit_code = code;
                block.duration = block.started.map(|t| t.elapsed());
            }
        }
    }

    /// Llamar después de procesar cada lectura: si se borró el historial (`cls`), los bloques ya no apuntan a nada.
    /// ponytail: al llenarse el historial (10.000 líneas) las líneas viejas se descartan y los bloques más
    /// antiguos quedan corridos; arreglarlo pide marcar las filas dentro de la grilla misma.
    pub(crate) fn after_output<T: EventListener>(&mut self, term: &Term<T>) {
        let history = term.grid().history_size();
        if history < self.history {
            self.list.clear();
        }
        self.history = history;
    }

    /// El bloque abierto (el último, si todavía no terminó).
    fn current(&mut self) -> Option<&mut Block> {
        self.list.last_mut().filter(|b| b.end.is_none())
    }
}

/// Número de línea lógica de una fila: cuántas líneas de texto "reales" hay antes, contando el historial.
///
/// Una línea larga ocupa varias filas, y la celda final de cada fila cortada tiene el flag WRAPLINE
/// ("sigue abajo"). Al achicar la ventana, alacritty vuelve a cortar esas líneas y las filas se corren,
/// pero la cantidad de líneas lógicas no cambia: por eso los bloques guardan esto y no el número de fila.
pub(crate) fn logical_line<T>(term: &Term<T>, line: Line) -> usize {
    let grid = term.grid();
    let last = grid.last_column();
    (grid.topmost_line().0..line.0)
        .filter(|&l| !grid[Line(l)][last].flags.contains(Flags::WRAPLINE))
        .count()
}

/// Lo inverso de `logical_line`: la primera fila de la grilla donde empieza la línea lógica `logical`.
pub(crate) fn grid_line_of<T>(term: &Term<T>, logical: usize) -> Option<Line> {
    let grid = term.grid();
    let last = grid.last_column();
    let mut current = 0;
    for l in grid.topmost_line().0..=grid.bottommost_line().0 {
        if current == logical {
            return Some(Line(l));
        }
        if !grid[Line(l)][last].flags.contains(Flags::WRAPLINE) {
            current += 1;
        }
    }
    None
}

/// Fila contando desde la más vieja del historial (no cambia al llegar líneas nuevas, mientras quepan).
fn absolute_row<T>(term: &Term<T>, line: Line) -> usize {
    (line.0 + term.grid().history_size() as i32) as usize
}

fn grid_line<T>(term: &Term<T>, row: usize) -> Line {
    Line(row as i32 - term.grid().history_size() as i32)
}

/// La celda anterior a `point` (la última de la fila de arriba si `point` está en la columna 0).
fn before<T>(term: &Term<T>, point: Point) -> Option<Point> {
    if point.column.0 > 0 {
        Some(Point::new(point.line, point.column - 1))
    } else if point.line > term.grid().topmost_line() {
        Some(Point::new(point.line - 1, term.grid().last_column()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::term_size;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    /// Pasa bytes por el escáner, el parser y los bloques, como hace la app.
    fn run(term: &mut Term<VoidListener>, blocks: &mut Blocks, chunks: &[&[u8]]) {
        let mut scanner = MarkScanner::default();
        let mut processor: Processor = Processor::new();
        for chunk in chunks {
            for piece in scanner.feed(chunk) {
                match piece {
                    Piece::Data(bytes) => processor.advance(term, &bytes),
                    Piece::Mark(mark) => blocks.on_mark(mark, term),
                }
            }
            blocks.after_output(term);
        }
    }

    #[test]
    fn scanner_extracts_marks_even_when_split() {
        let mut s = MarkScanner::default();
        // Una marca partida en tres lecturas, con texto antes y después.
        assert_eq!(s.feed(b"hola\x1b]13"), [Piece::Data(b"hola".to_vec())]);
        assert_eq!(s.feed(b"3;D;"), []);
        assert_eq!(
            s.feed(b"42\x07chao"),
            [
                Piece::Mark(Mark::Finished(Some(42))),
                Piece::Data(b"chao".to_vec())
            ]
        );
        // Terminador ST (ESC \) y marca sin código.
        assert_eq!(
            s.feed(b"\x1b]133;D\x1b\\"),
            [Piece::Mark(Mark::Finished(None))]
        );
        // Otras secuencias pasan intactas, aunque se parezcan (OSC 0 = título; ESC[ = CSI).
        let other = b"\x1b]0;titulo\x07\x1b[31mrojo";
        assert_eq!(s.feed(other), [Piece::Data(other.to_vec())]);
        // Una lectura que termina en ESC queda pendiente hasta ver qué sigue.
        assert_eq!(s.feed(b"x\x1b"), [Piece::Data(b"x".to_vec())]);
        assert_eq!(s.feed(b"[0m"), [Piece::Data(b"\x1b[0m".to_vec())]);
    }

    #[test]
    fn blocks_record_command_output_and_exit_code() {
        let mut term = Term::new(Config::default(), &term_size((40, 10)), VoidListener);
        let mut blocks = Blocks::default();
        let prompt: &[u8] = b"\x1b]133;A\x07PS> \x1b]133;B\x07";
        run(
            &mut term,
            &mut blocks,
            &[
                prompt,
                b"echo hola\r\n\x1b]133;C\x07hola\r\nmundo\r\n\x1b]133;D;0\x07",
                prompt,
                b"\r\n\x1b]133;D\x07", // Enter vacío: no debe crear bloque
                prompt,
                b"cmd /c exit 3\r\n\x1b]133;C\x07\x1b]133;D;3\x07",
                prompt,
            ],
        );
        let done: Vec<_> = blocks.list.iter().filter(|b| b.end.is_some()).collect();
        assert_eq!(done.len(), 2);
        let echo = done[0];
        assert_eq!(echo.command, "echo hola");
        assert_eq!((echo.prompt, echo.output, echo.end), (0, Some(1), Some(3)));
        assert_eq!(echo.exit_code, Some(0));
        assert!(echo.duration.is_some());
        // La salida se puede copiar tal cual: dos líneas.
        assert_eq!(echo.output_text(&term).as_deref(), Some("hola\nmundo"));
        assert_eq!(echo.output_lines(), 2);
        // `cmd /c exit 3` no imprimió nada.
        assert_eq!(done[1].output_text(&term).as_deref(), Some(""));
        assert_eq!(done[1].command, "cmd /c exit 3");
        assert_eq!(done[1].exit_code, Some(3));
        // El último prompt queda abierto, esperando un comando.
        assert_eq!(blocks.list.last().unwrap().end, None);
    }

    #[test]
    fn logical_lines_survive_resize() {
        let mut term = Term::new(Config::default(), &term_size((10, 6)), VoidListener);
        let mut processor: Processor = Processor::new();
        // 25 letras en 10 columnas = 3 filas, pero 1 sola línea lógica; después, otra línea.
        processor.advance(&mut term, b"abcdefghijklmnopqrstuvwxy\r\nfin");
        let cursor = || term.grid().cursor.point.line;
        assert_eq!(cursor(), Line(3));
        assert_eq!(logical_line(&term, cursor()), 1);
        // Más ancho: la línea larga ahora cabe en 1 fila y "fin" sube a la fila 1.
        term.resize(term_size((40, 6)));
        let line = term.grid().cursor.point.line;
        assert_eq!(line, Line(1));
        assert_eq!(logical_line(&term, line), 1); // la línea lógica no cambió
    }

    #[test]
    fn real_powershell_produces_a_block() {
        use crate::pty::{pty_size, shell_args, spawn_reader, spawn_shell};
        use std::io::Write;
        let mut shell =
            spawn_shell(pty_size((80, 24)), &shell_args(&["-NoProfile", "-NoLogo"])).unwrap();
        // Sin un Term que responda, contestamos nosotros la pregunta inicial de ConPTY (ESC[6n).
        shell.writer.borrow_mut().write_all(b"\x1b[1;1R").unwrap();
        let rx = spawn_reader(&*shell.master, || {}).unwrap();
        let mut term = Term::new(Config::default(), &term_size((80, 24)), VoidListener);
        let mut blocks = Blocks::default();
        let mut scanner = MarkScanner::default();
        let mut processor: Processor = Processor::new();
        let mut typed = false;
        let deadline = Instant::now() + Duration::from_secs(20);
        // Hasta que haya un bloque terminado (o se acabe el tiempo).
        while !blocks.list.iter().any(|b| b.end.is_some()) {
            let left = deadline.saturating_duration_since(Instant::now());
            let chunk = rx
                .recv_timeout(left)
                .expect("PowerShell no terminó el bloque a tiempo");
            for piece in scanner.feed(&chunk) {
                match piece {
                    Piece::Data(bytes) => processor.advance(&mut term, &bytes),
                    Piece::Mark(mark) => {
                        blocks.on_mark(mark, &term);
                        // Escribimos el comando recién cuando el primer prompt está listo (marca B).
                        if mark == Mark::CommandStart && !typed {
                            typed = true;
                            shell
                                .writer
                                .borrow_mut()
                                .write_all(b"echo orion-bloque\r")
                                .unwrap();
                        }
                    }
                }
            }
        }
        let block = blocks.list.iter().find(|b| b.end.is_some()).unwrap();
        assert_eq!(block.command, "echo orion-bloque");
        assert_eq!(block.exit_code, Some(0));
        let _ = shell.child.kill();
    }

    #[test]
    fn visible_rows_map_to_logical_lines() {
        let mut term = Term::new(Config::default(), &term_size((10, 5)), VoidListener);
        let mut processor: Processor = Processor::new();
        // "uno", una línea de 15 letras (2 filas), "tres".
        processor.advance(&mut term, b"uno\r\nabcdefghijklmno\r\ntres");
        let lines = visible_logical_lines(&term);
        assert_eq!(lines, [0, 1, 1, 2, 3]); // la fila 2 sigue siendo la línea lógica 1
        // Un bloque en las líneas lógicas [1, 3) ocupa las filas 1 a 3.
        assert_eq!(visible_rows_of(&lines, (1, 3)), Some((1, 3)));
        assert_eq!(visible_rows_of(&lines, (7, 9)), None); // fuera de la vista
    }

    #[test]
    fn durations_read_well() {
        let ms = Duration::from_millis;
        assert_eq!(format_duration(ms(120)), "120 ms");
        assert_eq!(format_duration(ms(2340)), "2.3 s");
        assert_eq!(format_duration(ms(65_000)), "1 min 5 s");
    }

    #[test]
    fn status_follows_the_block_lifecycle() {
        let mut term = Term::new(Config::default(), &term_size((40, 10)), VoidListener);
        let mut blocks = Blocks::default();
        let status = |b: &Blocks| b.list.last().unwrap().status();
        run(
            &mut term,
            &mut blocks,
            &[b"\x1b]133;A\x07$ \x1b]133;B\x07ls"],
        );
        assert_eq!(status(&blocks), Status::Editing);
        run(&mut term, &mut blocks, &[b"\r\n\x1b]133;C\x07"]);
        assert!(matches!(status(&blocks), Status::Running(_)));
        run(&mut term, &mut blocks, &[b"\x1b]133;D;1\x07"]);
        assert!(matches!(status(&blocks), Status::Done(Some(1), _)));
    }

    #[test]
    fn clearing_history_drops_blocks() {
        let mut term = Term::new(Config::default(), &term_size((20, 3)), VoidListener);
        let mut blocks = Blocks::default();
        let lines = b"1\r\n2\r\n3\r\n4\r\n5\r\n";
        run(&mut term, &mut blocks, &[b"\x1b]133;A\x07$ ", lines]);
        assert_eq!(blocks.list.len(), 1);
        // ESC[3J borra el historial (lo hace `cls`).
        run(&mut term, &mut blocks, &[b"\x1b[3J"]);
        assert!(blocks.list.is_empty());
    }
}
