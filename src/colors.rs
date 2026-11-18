//! Colores: tema, paleta ANSI de 16/256 colores, truecolor y contraste mínimo.

use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::Color;
use alacritty_terminal::vte::ansi::NamedColor;
use alacritty_terminal::vte::ansi::Rgb;
use eframe::egui::Color32;

// Tema: Tokyo Night. ponytail: fijo hasta tener temas en el config (#25).
pub(crate) const FG: Color32 = Color32::from_rgb(0xc0, 0xca, 0xf5);
pub(crate) const BG: Color32 = Color32::from_rgb(0x1a, 0x1b, 0x26);

/// Los 16 colores ANSI: 0-7 normales, 8-15 brillantes.
pub(crate) const PALETTE: [Color32; 16] = [
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
pub(crate) fn dim(c: Color32) -> Color32 {
    let f = |v: u8| (v as f32 * 2.0 / 3.0) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// Color por índice cuando la app no lo redefinió. Numeración de xterm + los extras de Alacritty.
pub(crate) fn default_color(idx: usize) -> Color32 {
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
pub(crate) fn resolve(color: Color, colors: &Colors) -> Color32 {
    let idx = match color {
        Color::Spec(Rgb { r, g, b }) => return Color32::from_rgb(r, g, b), // truecolor
        Color::Indexed(i) => i as usize,
        Color::Named(n) => n as usize,
    };
    // Si la app redefinió ese color (OSC 4/10/11), manda eso; si no, nuestra paleta.
    colors[idx].map_or_else(|| default_color(idx), |c| Color32::from_rgb(c.r, c.g, c.b))
}

/// Contraste mínimo entre texto y un fondo de color (escala WCAG: 1 = igual, 21 = negro/blanco).
/// 3:1 es el mínimo legible para texto; por debajo, cambiamos el texto a oscuro o claro.
pub(crate) const MIN_CONTRAST: f64 = 3.0;

/// Si `fg` casi no se distingue de `bg`, lo reemplaza por el fondo o el texto del tema, el que contraste más.
/// Ej.: PowerShell pinta carpetas con fondo azul (ESC[44m); el azul de Tokyo Night es claro y el texto claro no se lee.
pub(crate) fn readable(fg: Color32, bg: Color32) -> Color32 {
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

/// Colores finales (texto, fondo) de una celda, ya aplicados dim, inverso y oculto.
pub(crate) fn cell_colors(cell: &Cell, colors: &Colors) -> (Color32, Color32) {
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

/// Fondo del texto seleccionado (selección de Tokyo Night).
pub(crate) const SELECTION: Color32 = Color32::from_rgb(0x33, 0x46, 0x7c);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::term_size;
    use crate::grid::visible_rows;
    use alacritty_terminal::Term;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

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
}
