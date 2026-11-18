//! Geometría de la grilla: qué celdas se ven, cuántas caben y qué celda hay bajo el mouse.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Column;
use alacritty_terminal::index::Line;
use alacritty_terminal::index::Point;
use alacritty_terminal::index::Side;
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use eframe::egui;

/// Celdas visibles, una fila por línea de pantalla. El índice dentro de la fila es la columna.
pub(crate) fn visible_rows<T: EventListener>(term: &Term<T>) -> Vec<Vec<&Cell>> {
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
pub(crate) fn is_spacer(cell: &Cell) -> bool {
    cell.flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
}

/// ponytail: TermSize vive en `term::test` pero es público y es justo lo que Term::new/resize piden.
pub(crate) fn term_size((cols, rows): (u16, u16)) -> TermSize {
    TermSize::new(cols as usize, rows as usize)
}

/// Píxel → celda de la grilla, más la mitad de la celda (izq./der.) donde cayó el mouse.
/// `display_offset` traduce la fila en pantalla a la línea real cuando miras el historial.
pub(crate) fn pixel_to_point(
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

/// Cuántas celdas de tamaño `cell` caben en `area`. Nunca menos de 1x1: un PTY de 0 columnas falla.
pub(crate) fn grid_size(area: egui::Vec2, cell: egui::Vec2) -> (u16, u16) {
    let fit = |a: f32, c: f32| (a / c).floor().clamp(1.0, u16::MAX as f32) as u16;
    (fit(area.x, cell.x), fit(area.y, cell.y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::selection::Selection;
    use alacritty_terminal::selection::SelectionType;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

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
