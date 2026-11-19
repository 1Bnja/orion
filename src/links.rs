//! Links y rutas en la salida: Ctrl+clic abre las URLs en el navegador y las rutas
//! (`src/app.rs:42:5`, `app.ts(12,5)`) en el editor, en esa línea.

use alacritty_terminal::term::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Abre una URL o un archivo con lo que el sistema tenga predeterminado.
#[cfg(windows)]
const OPENER: &str = "explorer.exe";
#[cfg(not(windows))]
const OPENER: &str = "open";
/// VS Code desde la línea de comandos (en Windows es un .cmd).
#[cfg(windows)]
const CODE: &str = "code.cmd";
#[cfg(not(windows))]
const CODE: &str = "code";

#[derive(Debug, PartialEq)]
pub(crate) enum Link {
    Url(String),
    File {
        path: PathBuf,
        line: Option<u32>,
        col: Option<u32>,
    },
}

/// El link bajo la columna `col` de una fila, con las columnas que ocupa.
/// Las rutas solo cuentan si el archivo existe (relativas a `cwd`).
pub(crate) fn link_at(
    cells: &[&Cell],
    col: usize,
    cwd: Option<&Path>,
) -> Option<(Range<usize>, Link)> {
    // OSC 8: el programa marcó el texto como link. Ocupa las celdas seguidas con el mismo link.
    if let Some(link) = cells.get(col)?.hyperlink() {
        let same = |c: &&&Cell| c.hyperlink().as_ref() == Some(&link);
        let start = col - cells[..col].iter().rev().take_while(same).count();
        let end = col + cells[col..].iter().take_while(same).count();
        let url = link.uri();
        return is_web(url).then(|| (start..end, Link::Url(url.to_string())));
    }

    // Si no, la "palabra" bajo el mouse (hasta un espacio o comillas).
    let chars: Vec<char> = cells.iter().map(|c| c.c).collect();
    let stop = |c: char| c.is_whitespace() || "\"'`<>|".contains(c);
    if stop(chars[col]) {
        return None;
    }
    let mut start = col - chars[..col].iter().rev().take_while(|c| !stop(**c)).count();
    let mut end = col + chars[col..].iter().take_while(|c| !stop(**c)).count();
    // Sin la puntuación que la rodea: "(ver src/a.rs)." → src/a.rs
    while start < end && "([{".contains(chars[start]) {
        start += 1;
    }
    while end > start
        && ".,;:!?)]}".contains(chars[end - 1])
        && !(chars[end - 1] == ')' && chars[start..end].contains(&'('))
    {
        end -= 1;
    }
    if !(start..end).contains(&col) {
        return None;
    }
    let word: String = chars[start..end].iter().collect();
    if let Some(at) = word.find("https://").or_else(|| word.find("http://")) {
        return Some((start + at..end, Link::Url(word[at..].to_string())));
    }
    let (file, line, col) = split_location(&word);
    let path = cwd.unwrap_or(Path::new(".")).join(file);
    path.is_file()
        .then_some((start..end, Link::File { path, line, col }))
}

fn is_web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// "a.rs:42:5" → ("a.rs", 42, 5). También "a.ts(12,5)" (TypeScript, C#).
/// Ojo con "C:\x": el ":" de la unidad no es una línea.
fn split_location(word: &str) -> (&str, Option<u32>, Option<u32>) {
    if let Some((file, rest)) = word.strip_suffix(')').and_then(|w| w.rsplit_once('(')) {
        let mut nums = rest.split(',').map(|n| n.trim().parse().ok());
        if let Some(Some(line)) = nums.next() {
            return (file, Some(line), nums.next().flatten());
        }
    }
    let mut parts: Vec<&str> = word.rsplitn(3, ':').collect();
    parts.reverse(); // [archivo, línea, columna] o menos
    let num = |s: &str| s.parse::<u32>().ok();
    match parts.as_slice() {
        [file, line, col] if num(line).is_some() && num(col).is_some() => {
            (file, num(line), num(col))
        }
        [.., line] if num(line).is_some() => {
            let file = &word[..word.len() - line.len() - 1];
            (file, num(line), None)
        }
        _ => (word, None, None),
    }
}

/// Abre el link sin congelar la ventana.
pub(crate) fn open(link: Link) {
    std::thread::spawn(move || match link {
        // explorer (o `open` en Mac) abre las URLs con el navegador predeterminado.
        Link::Url(url) => {
            let _ = Command::new(OPENER).arg(url).spawn();
        }
        Link::File { path, line, col } => {
            // El editor: VS Code (`code -g archivo:línea:columna`). Si no está, la app predeterminada.
            let target = match (line, col) {
                (Some(l), Some(c)) => format!("{}:{l}:{c}", path.display()),
                (Some(l), None) => format!("{}:{l}", path.display()),
                _ => path.display().to_string(),
            };
            let code = crate::update::quiet(CODE).args(["-g", &target]).status();
            if !code.is_ok_and(|s| s.success()) {
                let _ = Command::new(OPENER).arg(&path).spawn();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> Vec<Cell> {
        text.chars()
            .map(|c| Cell {
                c,
                ..Default::default()
            })
            .collect()
    }

    fn at(text: &str, col: usize, cwd: &Path) -> Option<(Range<usize>, Link)> {
        let cells = row(text);
        let refs: Vec<&Cell> = cells.iter().collect();
        link_at(&refs, col, Some(cwd))
    }

    #[test]
    fn urls_and_locations_are_found() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let url = at("ver (https://example.com/a?b=1).", 10, here).unwrap();
        assert_eq!(url, (5..30, Link::Url("https://example.com/a?b=1".into())));

        let file = at("error en src/main.rs:12:5: algo", 12, here).unwrap();
        let path = here.join("src/main.rs");
        assert_eq!(
            file,
            (
                9..25,
                Link::File {
                    path: path.clone(),
                    line: Some(12),
                    col: Some(5)
                }
            )
        );
        assert_eq!(
            at("src/main.rs(7,2)", 0, here).unwrap().1,
            Link::File {
                path: path.clone(),
                line: Some(7),
                col: Some(2)
            }
        );
        assert_eq!(
            at("src/main.rs:3", 0, here).unwrap().1,
            Link::File {
                path,
                line: Some(3),
                col: None
            }
        );

        // Una palabra que no es un archivo, o un espacio, no es link.
        assert_eq!(at("hola mundo", 1, here), None);
        assert_eq!(at("hola mundo", 4, here), None);
        assert_eq!(
            split_location(r"C:\x\a.rs:9"),
            (r"C:\x\a.rs", Some(9), None)
        );
        assert_eq!(split_location(r"C:\x\a.rs"), (r"C:\x\a.rs", None, None));
    }
}
