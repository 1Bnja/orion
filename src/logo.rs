//! El logo de cada agente: qué dibujo le toca y cómo se convierte en píxeles.
//!
//! Los SVG de `assets/agents` son blancos y de un solo `<path>` hecho de rectas, así que no hace
//! falta una librería de SVG: se rasterizan aquí, al tamaño exacto en píxeles que se va a ver
//! (nítidos con cualquier escala de pantalla). Al dibujarlos se tiñen con el color del tema.

/// Un logo de `assets/agents`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Logo {
    Claude,
    Gemini,
    Codex,
    OpenCode,
    Aider,
    /// Cualquier otro agente: una ventana de terminal.
    Generic,
}

/// Lado del `viewBox` de los SVG.
const VIEW: f32 = 24.0;
/// Muestras por lado en cada píxel, para suavizar los bordes.
const SAMPLES: u32 = 4;

impl Logo {
    fn svg(self) -> &'static str {
        match self {
            Self::Claude => include_str!("../assets/agents/claude-code.svg"),
            Self::Gemini => include_str!("../assets/agents/gemini.svg"),
            Self::Codex => include_str!("../assets/agents/codex.svg"),
            Self::OpenCode => include_str!("../assets/agents/opencode.svg"),
            Self::Aider => include_str!("../assets/agents/aider.svg"),
            Self::Generic => include_str!("../assets/agents/generic.svg"),
        }
    }

    /// El logo como cuadrado de `side` px: cuánto cubre el dibujo cada píxel (0 a 255),
    /// fila por fila.
    pub(crate) fn coverage(self, side: usize) -> Vec<u8> {
        let shapes = polygons(self.svg());
        let step = VIEW / (side as f32 * SAMPLES as f32);
        let mut out = Vec::with_capacity(side * side);
        for row in 0..side as u32 {
            for col in 0..side as u32 {
                let mut hits = 0;
                for sy in 0..SAMPLES {
                    for sx in 0..SAMPLES {
                        let x = ((col * SAMPLES + sx) as f32 + 0.5) * step;
                        let y = ((row * SAMPLES + sy) as f32 + 0.5) * step;
                        hits += u32::from(winding(&shapes, x, y) != 0);
                    }
                }
                out.push((hits * 255 / (SAMPLES * SAMPLES)) as u8);
            }
        }
        out
    }
}

/// Los polígonos del `d="…"` de uno de nuestros SVG: solo `M`, `L` y `Z`, separados por espacios.
/// Los archivos van dentro del programa y hay un test que los revisa todos.
fn polygons(svg: &str) -> Vec<Vec<(f32, f32)>> {
    let data = svg.split(" d=\"").nth(1).and_then(|s| s.split('"').next());
    let mut shapes: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut x = None;
    for token in data.unwrap_or_default().split_whitespace() {
        match token {
            "M" => shapes.push(Vec::new()),
            "L" | "Z" => {}
            number => {
                let value: f32 = number.parse().expect("número en el SVG de un logo");
                match x.take() {
                    None => x = Some(value),
                    Some(x) => shapes.last_mut().expect("empieza con M").push((x, value)),
                }
            }
        }
    }
    shapes
}

/// Cuántas vueltas dan los polígonos alrededor del punto (la regla "nonzero" de SVG).
fn winding(shapes: &[Vec<(f32, f32)>], x: f32, y: f32) -> i32 {
    let mut turns = 0;
    for points in shapes {
        for (i, &(ax, ay)) in points.iter().enumerate() {
            let (bx, by) = points[(i + 1) % points.len()];
            if (ay > y) != (by > y) && x < ax + (y - ay) / (by - ay) * (bx - ax) {
                turns += if by > ay { 1 } else { -1 };
            }
        }
    }
    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Logo; 6] = [
        Logo::Claude,
        Logo::Gemini,
        Logo::Codex,
        Logo::OpenCode,
        Logo::Aider,
        Logo::Generic,
    ];

    #[test]
    fn every_svg_draws_something_different_inside_its_box() {
        let images = ALL.map(|logo| logo.coverage(28));
        for (logo, image) in ALL.iter().zip(&images) {
            for shape in polygons(logo.svg()) {
                assert!(shape.len() >= 3, "{logo:?}");
                let inside = |v: f32| (0.0..=VIEW).contains(&v);
                assert!(shape.iter().all(|&(x, y)| inside(x) && inside(y)));
            }
            let inked = image.iter().filter(|&&a| a > 128).count();
            assert!((60..600).contains(&inked), "{logo:?}: {inked}");
            assert_eq!(image[0], 0, "{logo:?}: la esquina va vacía");
        }
        for (i, image) in images.iter().enumerate() {
            assert!(!images[i + 1..].contains(image), "{:?}", ALL[i]);
        }
    }

    #[test]
    fn holes_stay_empty_and_overlaps_stay_filled() {
        let at = |logo: Logo, x: usize, y: usize| logo.coverage(24)[y * 24 + x];
        assert_eq!(at(Logo::OpenCode, 12, 7), 0); // el hueco del marco
        assert_eq!(at(Logo::OpenCode, 12, 14), 255); // el bloque dentro del hueco
        assert_eq!(at(Logo::Aider, 12, 11), 0); // el triángulo de la A
        assert_eq!(at(Logo::Claude, 12, 12), 255); // el centro del destello
    }
}
