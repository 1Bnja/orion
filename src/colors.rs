//! Colores: temas, paleta ANSI de 16/256 colores, truecolor y contraste mínimo.

use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::Color;
use alacritty_terminal::vte::ansi::NamedColor;
use alacritty_terminal::vte::ansi::Rgb;
use eframe::egui::Color32;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

/// Un tema: el texto, el fondo, la selección y los 16 colores ANSI (0-7 normales, 8-15 brillantes).
/// Los colores de la interfaz (barra lateral, separadores) se sacan mezclando el fondo y el texto.
pub(crate) struct Theme {
    pub(crate) name: &'static str,
    pub(crate) fg: Color32,
    pub(crate) bg: Color32,
    pub(crate) selection: Color32,
    pub(crate) ansi: [Color32; 16],
}

/// 0xRRGGBB → Color32 (const: así los temas se escriben como en sus sitios oficiales).
const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

const fn ansi(c: [u32; 16]) -> [Color32; 16] {
    let mut out = [Color32::BLACK; 16];
    let mut i = 0;
    while i < 16 {
        out[i] = hex(c[i]);
        i += 1;
    }
    out
}

/// Los temas incluidos. El primero es el de siempre.
pub(crate) static THEMES: [Theme; 5] = [
    Theme {
        name: "Tokyo Night",
        fg: hex(0xc0caf5),
        bg: hex(0x1a1b26),
        selection: hex(0x33467c),
        ansi: ansi([
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, //
            0x414868, 0xff899d, 0x9fe044, 0xfaba4a, 0x8db0ff, 0xc7a9ff, 0xa4daff, 0xc0caf5,
        ]),
    },
    Theme {
        name: "Catppuccin Mocha",
        fg: hex(0xcdd6f4),
        bg: hex(0x1e1e2e),
        selection: hex(0x45475a),
        ansi: ansi([
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, //
            0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
        ]),
    },
    Theme {
        name: "Dracula",
        fg: hex(0xf8f8f2),
        bg: hex(0x282a36),
        selection: hex(0x44475a),
        ansi: ansi([
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, //
            0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ]),
    },
    Theme {
        name: "Gruvbox",
        fg: hex(0xebdbb2),
        bg: hex(0x282828),
        selection: hex(0x504945),
        ansi: ansi([
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, //
            0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ]),
    },
    Theme {
        name: "Catppuccin Latte",
        fg: hex(0x4c4f69),
        bg: hex(0xeff1f5),
        selection: hex(0xacb0be),
        ansi: ansi([
            0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, //
            0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
        ]),
    },
];

/// Índice en THEMES del tema actual.
/// ponytail: global porque se lee en todos lados al pintar; un índice atómico basta (solo lo cambia la UI).
static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// El tema actual.
pub(crate) fn theme() -> &'static Theme {
    &THEMES[CURRENT.load(Ordering::Relaxed)]
}

/// Índice del tema con ese nombre (sin importar mayúsculas).
pub(crate) fn find_theme(name: &str) -> Option<usize> {
    THEMES
        .iter()
        .position(|t| t.name.eq_ignore_ascii_case(name.trim()))
}

pub(crate) fn set_theme(index: usize) {
    CURRENT.store(index.min(THEMES.len() - 1), Ordering::Relaxed);
}

/// Opacidad de los fondos de Orion (255 = opaco). Menos de 255 deja ver Mica o el acrílico detrás.
static SURFACE_ALPHA: AtomicU8 = AtomicU8::new(255);

pub(crate) fn set_surface_alpha(alpha: u8) {
    SURFACE_ALPHA.store(alpha, Ordering::Relaxed);
}

/// ¿Los fondos son opacos? (sin Mica ni acrílico).
pub(crate) fn opaque() -> bool {
    SURFACE_ALPHA.load(Ordering::Relaxed) == 255
}

/// Un color de fondo (la terminal, la barra lateral) con la opacidad actual.
pub(crate) fn surface(c: Color32) -> Color32 {
    let alpha = SURFACE_ALPHA.load(Ordering::Relaxed);
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

/// `a` llevado hacia `b` en una fracción `t` (0 = a, 1 = b).
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

impl Theme {
    /// ¿Fondo claro? (para elegir los colores de las ventanas de egui).
    pub(crate) fn is_light(&self) -> bool {
        let c = self.bg;
        0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32 > 128.0
    }

    /// Barra lateral: un poco más oscura que la terminal.
    pub(crate) fn sidebar(&self) -> Color32 {
        mix(self.bg, Color32::BLACK, 0.15)
    }

    /// Fondo de una fila bajo el mouse.
    pub(crate) fn hover(&self) -> Color32 {
        mix(self.bg, self.fg, 0.05)
    }

    /// Líneas separadoras y bordes.
    pub(crate) fn line(&self) -> Color32 {
        mix(self.bg, self.fg, 0.12)
    }
}

/// "Dim" (SGR 2) = el color a 2/3 de intensidad.
pub(crate) fn dim(c: Color32) -> Color32 {
    let f = |v: u8| (v as f32 * 2.0 / 3.0) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// Color por índice cuando la app no lo redefinió. Numeración de xterm + los colores con nombre de alacritty_terminal.
pub(crate) fn default_color(idx: usize) -> Color32 {
    // El cubo 6x6x6 usa estos 6 niveles por canal.
    let level = |v: usize| if v == 0 { 0 } else { (55 + 40 * v) as u8 };
    match idx {
        0..16 => theme().ansi[idx],
        16..232 => {
            let i = idx - 16;
            Color32::from_rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        232..256 => {
            let g = (8 + 10 * (idx - 232)) as u8; // 24 grises de 8 a 238
            Color32::from_rgb(g, g, g)
        }
        _ => match idx {
            i if i == NamedColor::Background as usize => theme().bg,
            i if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
                dim(theme().ansi[i - NamedColor::DimBlack as usize])
            }
            i if i == NamedColor::DimForeground as usize => dim(theme().fg),
            _ => theme().fg, // Foreground, Cursor, BrightForeground
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
    if contrast(theme().bg) > contrast(theme().fg) {
        theme().bg
    } else {
        theme().fg
    }
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
    if bg != theme().bg {
        fg = readable(fg, bg);
    }
    if cell.flags.contains(Flags::HIDDEN) {
        fg = bg;
    }
    (fg, bg)
}

#[cfg(test)]
mod tests {
    use super::*;
    const BG: Color32 = THEMES[0].bg;
    const PALETTE: [Color32; 16] = THEMES[0].ansi;
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
