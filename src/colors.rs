//! Colores: temas, paleta ANSI de 16/256 colores, truecolor y contraste mínimo.

use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::Color;
use alacritty_terminal::vte::ansi::NamedColor;
use alacritty_terminal::vte::ansi::Rgb;
use eframe::egui;
use eframe::egui::Color32;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{OnceLock, RwLock};

/// Un tema: el texto, el fondo, la selección y los 16 colores ANSI (0-7 normales, 8-15 brillantes).
/// Los colores de la interfaz (barra lateral, separadores) se sacan mezclando el fondo y el texto.
pub(crate) struct Theme {
    pub(crate) name: &'static str,
    pub(crate) fg: Color32,
    pub(crate) bg: Color32,
    pub(crate) selection: Color32,
    pub(crate) ansi: [Color32; 16],
    /// Monochrome style (Orion / Vercel): muted UI chrome; color only when it means something
    /// (failed command, status rail). No bordered cards between commands.
    pub(crate) mono: bool,
    /// Cursor color. None = same as the text.
    pub(crate) cursor: Option<Color32>,
    /// Overrides del usuario de los colores derivados (None = se calculan del tema).
    pub(crate) derived: Derived,
}

/// Colores de interfaz que normalmente se derivan del tema y que el usuario puede fijar a mano.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Derived {
    pub(crate) sidebar: Option<Color32>,
    pub(crate) accent: Option<Color32>,
    pub(crate) line: Option<Color32>,
    pub(crate) hover: Option<Color32>,
}

impl Derived {
    pub(crate) const NONE: Self = Self {
        sidebar: None,
        accent: None,
        line: None,
        hover: None,
    };
}

/// Cambios de color del usuario sobre el tema elegido. None = el color del tema.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) struct Overrides {
    pub(crate) fg: Option<Color32>,
    pub(crate) bg: Option<Color32>,
    pub(crate) selection: Option<Color32>,
    pub(crate) sidebar: Option<Color32>,
    pub(crate) accent: Option<Color32>,
    pub(crate) line: Option<Color32>,
    pub(crate) hover: Option<Color32>,
    pub(crate) ansi: [Option<Color32>; 16],
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
pub(crate) static THEMES: [Theme; 7] = [
    // Orion's own theme: the logo's near-black and a soft warm ivory (a hint of yellow, never pure white).
    Theme {
        name: "Orion",
        fg: hex(0xf8f5f0),
        bg: hex(0x020204),
        selection: hex(0x2c2a27),
        mono: true,
        cursor: Some(hex(0xe8d9c0)),
        derived: Derived::NONE,
        ansi: ansi([
            0x1a1918, 0xe8908a, 0x8fbf9a, 0xe0c48a, 0x8fadcf, 0xb89ec8, 0x7eb8b4, 0xb5b0a8, //
            0x6e6a64, 0xf0a8a2, 0xa8d0b2, 0xedd4a0, 0xa8c4de, 0xceb4dc, 0x96ccc8, 0xf8f5f0,
        ]),
    },
    // Inspirado en Vercel (Geist): negro puro, grises y el color solo cuando significa algo.
    Theme {
        name: "Vercel",
        fg: hex(0xededed),
        bg: hex(0x000000),
        selection: hex(0x333333),
        mono: true,
        cursor: None,
        derived: Derived::NONE,
        ansi: ansi([
            0x242424, 0xff6166, 0x4cc38a, 0xffb224, 0x52a8ff, 0xbf7af0, 0x0ac7b4, 0xa1a1a1, //
            0x8f8f8f, 0xff8a8e, 0x6fdba3, 0xffc53d, 0x75bfff, 0xd09cf6, 0x2fd8c5, 0xededed,
        ]),
    },
    Theme {
        name: "Tokyo Night",
        fg: hex(0xc0caf5),
        bg: hex(0x1a1b26),
        selection: hex(0x33467c),
        mono: false,
        cursor: None,
        derived: Derived::NONE,
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
        mono: false,
        cursor: None,
        derived: Derived::NONE,
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
        mono: false,
        cursor: None,
        derived: Derived::NONE,
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
        mono: false,
        cursor: None,
        derived: Derived::NONE,
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
        mono: false,
        cursor: None,
        derived: Derived::NONE,
        ansi: ansi([
            0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, //
            0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
        ]),
    },
];

/// El tema activo: el base elegido con los overrides del usuario encima.
/// ponytail: global porque se lee en todos lados al pintar; un RwLock basta (solo lo cambia la UI).
static ACTIVE: OnceLock<RwLock<&'static Theme>> = OnceLock::new();

fn active() -> &'static RwLock<&'static Theme> {
    ACTIVE.get_or_init(|| RwLock::new(&THEMES[0]))
}

/// El tema actual.
pub(crate) fn theme() -> &'static Theme {
    *active().read().unwrap_or_else(|e| e.into_inner())
}

/// Índice del tema con ese nombre (sin importar mayúsculas).
pub(crate) fn find_theme(name: &str) -> Option<usize> {
    THEMES
        .iter()
        .position(|t| t.name.eq_ignore_ascii_case(name.trim()))
}

/// Activa un tema ya resuelto (tema propio o built-in con overrides).
pub(crate) fn set_active_theme(theme: Theme) {
    let theme: &'static Theme = Box::leak(Box::new(theme));
    *active().write().unwrap_or_else(|e| e.into_inner()) = theme;
}

/// "#RRGGBB" (mayúsculas o minúsculas) → Color32. None si no es un hex de 6 dígitos.
pub(crate) fn parse_hex(text: &str) -> Option<Color32> {
    let digits = text.trim().strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(hex(u32::from_str_radix(digits, 16).ok()?))
}

/// Color32 → "#RRGGBB" (para mostrarlo y guardarlo en config.toml).
pub(crate) fn to_hex(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

/// Opacidad de los fondos de Orion (255 = opaco). Menos de 255 deja ver Mica o el acrílico detrás.
static SURFACE_ALPHA: AtomicU8 = AtomicU8::new(255);

pub(crate) fn set_surface_alpha(alpha: u8) {
    SURFACE_ALPHA.store(alpha, Ordering::Relaxed);
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
    /// Este tema con los overrides del usuario encima. Solo cambia lo que venga en `overrides`.
    pub(crate) fn with_overrides(&self, overrides: Overrides) -> Theme {
        let mut ansi = self.ansi;
        for (color, over) in ansi.iter_mut().zip(overrides.ansi) {
            if let Some(c) = over {
                *color = c;
            }
        }
        Theme {
            name: self.name,
            fg: overrides.fg.unwrap_or(self.fg),
            bg: overrides.bg.unwrap_or(self.bg),
            selection: overrides.selection.unwrap_or(self.selection),
            ansi,
            mono: self.mono,
            cursor: self.cursor,
            derived: Derived {
                sidebar: overrides.sidebar,
                accent: overrides.accent,
                line: overrides.line,
                hover: overrides.hover,
            },
        }
    }

    /// ¿Fondo claro? (para elegir los colores de las ventanas de egui).
    pub(crate) fn is_light(&self) -> bool {
        let c = self.bg;
        0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32 > 128.0
    }

    /// The frame around the terminal (title bar and sidebar): darker than the terminal, so the
    /// terminal reads as a lit panel. Pure black cannot go darker, so there it is a bit lighter.
    pub(crate) fn sidebar(&self) -> Color32 {
        if let Some(c) = self.derived.sidebar {
            return c;
        }
        if self.mono {
            mix(self.bg, self.fg, 0.06)
        } else if self.is_light() {
            mix(self.bg, Color32::BLACK, 0.08)
        } else {
            mix(self.bg, Color32::BLACK, 0.25)
        }
    }

    /// Text that matters less than the main text (inactive tabs, paths).
    pub(crate) fn secondary(&self) -> Color32 {
        mix(self.fg, self.bg, 0.28)
    }

    /// The quietest text that is still readable (hints, group names).
    pub(crate) fn muted(&self) -> Color32 {
        mix(self.fg, self.bg, 0.45)
    }

    pub(crate) fn cursor(&self) -> Color32 {
        self.cursor.unwrap_or(self.fg)
    }

    /// Fondo de una fila bajo el mouse.
    pub(crate) fn hover(&self) -> Color32 {
        self.derived
            .hover
            .unwrap_or_else(|| mix(self.bg, self.fg, 0.05))
    }

    /// Acento de la interfaz (pestaña activa, avisos): azul, o el color del texto si es monocromo.
    pub(crate) fn accent(&self) -> Color32 {
        if let Some(c) = self.derived.accent {
            return c;
        }
        if self.mono { self.fg } else { self.ansi[4] }
    }

    /// Líneas separadoras y bordes.
    pub(crate) fn line(&self) -> Color32 {
        self.derived
            .line
            .unwrap_or_else(|| mix(self.bg, self.fg, 0.12))
    }

    /// Lines and borders drawn on the frame (`sidebar`), which is not the color of the terminal.
    pub(crate) fn frame_line(&self) -> Color32 {
        self.derived
            .line
            .unwrap_or_else(|| mix(self.sidebar(), self.fg, 0.14))
    }

    /// La interfaz de egui (configuración, paleta, menús) con los colores del tema: el mismo
    /// fondo, el mismo texto y los mismos grises que la terminal. En los temas monocromos los
    /// botones van sin relleno hasta que el mouse los toca, como en Vercel.
    pub(crate) fn visuals(&self) -> egui::Visuals {
        let mut v = if self.is_light() {
            egui::Visuals::light()
        } else {
            egui::Visuals::dark()
        };
        let (bg, fg) = (self.bg, self.fg);
        let window = self.sidebar();
        let line = self.line();
        let hover = self.hover();
        let raised = mix(bg, fg, 0.10);
        let accent = self.accent();
        let widget = |bg_fill, weak_bg_fill, bg_stroke, fg_stroke, corner_radius| {
            egui::style::WidgetVisuals {
                bg_fill,
                weak_bg_fill,
                bg_stroke,
                corner_radius: egui::CornerRadius::same(corner_radius),
                fg_stroke,
                expansion: 0.0,
            }
        };

        v.override_text_color = Some(fg);
        v.window_fill = window;
        v.panel_fill = window;
        v.window_stroke = egui::Stroke::new(1.0, line);
        v.window_corner_radius = egui::CornerRadius::same(if self.mono { 10 } else { 6 });
        v.menu_corner_radius = egui::CornerRadius::same(8);
        v.extreme_bg_color = mix(bg, fg, 0.07); // fondo de los cuadros de texto
        v.faint_bg_color = hover;
        v.code_bg_color = raised;
        v.selection = egui::style::Selection {
            bg_fill: self.selection,
            stroke: egui::Stroke::new(1.0, fg),
        };
        v.hyperlink_color = accent;
        v.warn_fg_color = self.ansi[3];
        v.error_fg_color = self.ansi[1];
        v.text_cursor.stroke = egui::Stroke::new(2.0, fg);
        v.slider_trailing_fill = true;
        // Sombras suaves y centradas: la de egui cae en diagonal y se ve como un borde sucio.
        let shadow = |offset, blur, alpha: u8| egui::epaint::Shadow {
            offset: [0, offset],
            blur,
            spread: 0,
            color: Color32::from_black_alpha(if self.is_light() { alpha / 2 } else { alpha }),
        };
        v.window_shadow = shadow(12, 40, 110);
        v.popup_shadow = shadow(8, 24, 90);

        let w = &mut v.widgets;
        w.noninteractive = widget(
            window,
            window,
            egui::Stroke::new(1.0, line),
            egui::Stroke::new(1.0, fg),
            4,
        );
        w.inactive = widget(
            raised,
            Color32::TRANSPARENT,
            egui::Stroke::NONE,
            egui::Stroke::new(1.0, fg),
            6,
        );
        w.hovered = widget(
            mix(bg, fg, 0.16),
            hover,
            egui::Stroke::new(1.0, line),
            egui::Stroke::new(1.0, fg),
            6,
        );
        w.active = widget(
            mix(bg, fg, 0.20),
            raised,
            egui::Stroke::new(1.0, accent),
            egui::Stroke::new(1.0, fg),
            6,
        );
        w.open = widget(
            raised,
            hover,
            egui::Stroke::new(1.0, line),
            egui::Stroke::new(1.0, fg),
            6,
        );
        v
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
            i if i == NamedColor::Cursor as usize => theme().cursor(),
            _ => theme().fg, // Foreground, BrightForeground
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

    #[test]
    fn hex_round_trips_and_rejects_bad_values() {
        assert_eq!(
            parse_hex("#1a2B3c"),
            Some(Color32::from_rgb(0x1a, 0x2b, 0x3c))
        );
        assert_eq!(to_hex(Color32::from_rgb(1, 2, 255)), "#0102FF");
        for bad in ["1a2b3c", "#12345", "#12345g", "", "#"] {
            assert!(parse_hex(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn an_override_replaces_only_that_color() {
        let over = Overrides {
            fg: Some(Color32::from_rgb(1, 2, 3)),
            ansi: {
                let mut ansi = [None; 16];
                ansi[4] = Some(Color32::from_rgb(4, 5, 6));
                ansi
            },
            ..Overrides::default()
        };
        let t = THEMES[0].with_overrides(over);
        assert_eq!(t.fg, Color32::from_rgb(1, 2, 3));
        assert_eq!(t.ansi[4], Color32::from_rgb(4, 5, 6));
        assert_eq!(t.bg, THEMES[0].bg);
        assert_eq!(t.ansi[1], THEMES[0].ansi[1]);
        // Override de un color derivado: manda sobre el cálculo.
        let over = Overrides {
            sidebar: Some(Color32::from_rgb(9, 9, 9)),
            ..Overrides::default()
        };
        assert_eq!(
            THEMES[0].with_overrides(over).sidebar(),
            Color32::from_rgb(9, 9, 9)
        );
    }
}
