//! El ícono de Orion, dibujado con código: una órbita abierta (la "O" de Orion) que termina en una estrella.
//!
//! Cada figura es una "función de distancia" (SDF): para cada píxel, cuánto falta para llegar al borde
//! (negativo = adentro). Con eso el borde sale suave (antialiasing) en cualquier tamaño, de 16 a 256 px.
//! Se usa para el ícono de la ventana, con `to_ico` para generar assets/orion.ico (el del .exe) y con
//! `to_png` para el .icns de Orion.app (lo arma scripts/package-macos.sh).

use std::f32::consts::{PI, TAU};

/// RGBA lineal entre 0 y 1, con el alfa sin premultiplicar.
type Rgba = [f32; 4];

fn hex(v: u32, a: f32) -> Rgba {
    let c = |s: u32| ((v >> s) & 0xff) as f32 / 255.0;
    [c(16), c(8), c(0), a]
}

fn lerp(a: Rgba, b: Rgba, t: f32) -> Rgba {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// Pinta `top` encima de `bottom` ("over"), con `coverage` = cuánto del píxel cubre la figura.
fn over(bottom: Rgba, top: Rgba, coverage: f32) -> Rgba {
    let a = top[3] * coverage;
    let out_a = a + bottom[3] * (1.0 - a);
    if out_a <= 0.0 {
        return [0.0; 4];
    }
    let mut out = [0.0, 0.0, 0.0, out_a];
    for i in 0..3 {
        out[i] = (top[i] * a + bottom[i] * bottom[3] * (1.0 - a)) / out_a;
    }
    out
}

/// Distancia a un cuadrado de lado 2·`half` con esquinas de radio `r`, centrado en (0, 0).
fn rounded_square(x: f32, y: f32, half: f32, r: f32) -> f32 {
    let (qx, qy) = (x.abs() - half + r, y.abs() - half + r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Distancia a una estrella de 5 puntas de radio `r` (puntas) y `inner` (entrantes), con una punta hacia arriba.
fn star(x: f32, y: f32, r: f32, inner: f32) -> f32 {
    // Doblamos el plano en una de las 10 "porciones" de la estrella y medimos contra un solo segmento.
    let slice = TAU / 10.0;
    let angle = x.atan2(-y).rem_euclid(TAU); // 0 = arriba
    let a = (angle % (2.0 * slice) - slice).abs(); // 0 en una punta, slice en un entrante
    let d = x.hypot(y);
    let (px, py) = (d * a.cos(), d * a.sin());
    // Segmento de la punta (r, 0) al entrante (inner·cos, inner·sin).
    let (bx, by) = (inner * slice.cos(), inner * slice.sin());
    let (ex, ey) = (bx - r, by);
    let t = (((px - r) * ex + py * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
    let dist = (px - r - ex * t).hypot(py - ey * t);
    // ¿Adentro? Del lado del centro respecto del segmento.
    let side = (px - r) * ey - py * ex;
    if side < 0.0 { -dist } else { dist }
}

// Geometría, en unidades del ícono (0..1, y hacia abajo).
const CENTER: (f32, f32) = (0.47, 0.53);
const RADIUS: f32 = 0.29;
/// La órbita empieza arriba (ángulo de 80°, medido como en matemática) y gira hacia la izquierda
/// hasta la estrella, a la derecha (20°): una vuelta casi entera.
const ARC_START: f32 = 80.0 * PI / 180.0;
const ARC_END: f32 = (360.0 + 12.0) * PI / 180.0;
const STAR_ANGLE: f32 = 22.0 * PI / 180.0;

/// Punto de la órbita en el ángulo `a`.
fn orbit_point(a: f32) -> (f32, f32) {
    (CENTER.0 + RADIUS * a.cos(), CENTER.1 - RADIUS * a.sin())
}

/// Ancho del trazo en la fracción `t` de la órbita: fino al empezar, como la cola de un cometa.
fn orbit_width(t: f32) -> f32 {
    0.022 + 0.052 * t
}

/// Distancia a la órbita y en qué fracción del recorrido (0 = inicio, 1 = estrella) está lo más cercano.
fn orbit(x: f32, y: f32) -> (f32, f32) {
    let (dx, dy) = (x - CENTER.0, CENTER.1 - y);
    let span = ARC_END - ARC_START;
    // Ángulo del píxel, llevado al rango del arco.
    let mut a = dy.atan2(dx);
    while a < ARC_START {
        a += TAU;
    }
    let t = (a - ARC_START) / span;
    let ring = dx.hypot(dy) - RADIUS;
    if t <= 1.0 {
        return (ring.abs() - orbit_width(t) / 2.0, t);
    }
    // Fuera del arco: la distancia a la punta más cercana (puntas redondas).
    let cap = |angle: f32, t: f32| {
        let (px, py) = orbit_point(angle);
        ((x - px).hypot(y - py) - orbit_width(t) / 2.0, t)
    };
    let (start, end) = (cap(ARC_START, 0.0), cap(ARC_END, 1.0));
    if start.0 < end.0 { start } else { end }
}

/// Color de un punto del ícono (x, y en 0..1). `px` = tamaño de un píxel, para el antialiasing.
fn color_at(x: f32, y: f32, px: f32) -> Rgba {
    let coverage = |d: f32| (0.5 - d / px).clamp(0.0, 1.0);
    let mut c: Rgba = [0.0; 4];

    // Fondo: cuadrado redondeado del color de Tokyo Night, con un degradé apenas más claro arriba.
    let bg = lerp(hex(0x24283b, 1.0), hex(0x16161e, 1.0), y);
    let inside = coverage(rounded_square(x - 0.5, y - 0.5, 0.46, 0.2));
    c = over(c, bg, inside);

    // Órbita: de cian a violeta, como la estela que deja la estrella.
    let (d, t) = orbit(x, y);
    let trail = lerp(hex(0x7dcfff, 0.55), hex(0xbb9af7, 1.0), t.clamp(0.0, 1.0));
    c = over(c, trail, coverage(d));

    // Estrella al final de la órbita, con un brillo suave alrededor.
    let (sx, sy) = orbit_point(STAR_ANGLE);
    let glow_d = (x - sx).hypot(y - sy);
    c = over(c, hex(0xffd479, 0.45 * (-glow_d / 0.07).exp()), inside);
    let s = star(x - sx, y - sy, 0.115, 0.05);
    c = over(c, hex(0xffe7a8, 1.0), coverage(s));
    c
}

/// El ícono de `size`×`size` px, en RGBA de 8 bits (fila por fila, de arriba abajo).
pub(crate) fn render(size: u32) -> Vec<u8> {
    let px = 1.0 / size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for row in 0..size {
        for col in 0..size {
            // Centro del píxel.
            let c = color_at((col as f32 + 0.5) * px, (row as f32 + 0.5) * px, px);
            out.extend(c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
        }
    }
    out
}

/// Archivo .ico con el ícono en varios tamaños (Windows elige el que le sirve en cada lugar).
/// Cada imagen va como bitmap de 32 bits (BGRA, de abajo arriba) más una máscara AND vacía.
#[cfg(test)]
pub(crate) fn to_ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&size| {
            let rgba = render(size);
            let mut bmp = Vec::new();
            // BITMAPINFOHEADER: el alto es el doble (imagen + máscara), así lo pide el formato .ico.
            for v in [40u32, size, size * 2] {
                bmp.extend(v.to_le_bytes());
            }
            bmp.extend(1u16.to_le_bytes()); // planos
            bmp.extend(32u16.to_le_bytes()); // bits por píxel
            bmp.extend([0u8; 24]); // sin compresión y el resto en cero
            for row in (0..size).rev() {
                for col in 0..size {
                    let i = ((row * size + col) * 4) as usize;
                    bmp.extend([rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
                }
            }
            // Máscara AND: 1 bit por píxel, filas alineadas a 4 bytes. Toda en 0 (manda el alfa).
            let mask_row = size.div_ceil(32) * 4;
            bmp.extend(vec![0u8; (mask_row * size) as usize]);
            bmp
        })
        .collect();

    let mut ico = Vec::new();
    ico.extend(
        [0u16, 1, sizes.len() as u16]
            .iter()
            .flat_map(|v| v.to_le_bytes()),
    );
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&size, bmp) in sizes.iter().zip(&images) {
        let side = if size >= 256 { 0 } else { size as u8 }; // 0 significa 256
        ico.extend([side, side, 0, 0]);
        ico.extend(1u16.to_le_bytes());
        ico.extend(32u16.to_le_bytes());
        ico.extend((bmp.len() as u32).to_le_bytes());
        ico.extend(offset.to_le_bytes());
        offset += bmp.len() as u32;
    }
    for bmp in images {
        ico.extend(bmp);
    }
    ico
}

/// El ícono de `size`×`size` px como PNG (RGBA de 8 bits), para el .icns de macOS.
/// Sin comprimir (bloques "stored" de zlib): `scripts/package-macos.sh` lo recomprime con `sips`.
#[cfg(test)]
pub(crate) fn to_png(size: u32) -> Vec<u8> {
    let rgba = render(size);
    // Cada fila lleva adelante su tipo de filtro: 0 = ninguno.
    let mut raw = Vec::with_capacity(rgba.len() + size as usize);
    for row in rgba.chunks(size as usize * 4) {
        raw.push(0);
        raw.extend(row);
    }
    // zlib: cabecera, bloques de hasta 65535 bytes tal cual y el checksum Adler-32.
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == blocks.len())); // 1 = último bloque
        let len = block.len() as u16;
        zlib.extend(len.to_le_bytes());
        zlib.extend((!len).to_le_bytes());
        zlib.extend(*block);
    }
    zlib.extend(adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend(size.to_be_bytes());
    ihdr.extend(size.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]); // 8 bits, RGBA, compresión/filtro/entrelazado estándar
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(b"IHDR", ihdr), (b"IDAT", zlib), (b"IEND", Vec::new())] {
        png.extend((data.len() as u32).to_be_bytes());
        let start = png.len();
        png.extend(kind);
        png.extend(&data);
        png.extend(crc32(&png[start..]).to_be_bytes()); // el CRC cubre el tipo y los datos
    }
    png
}

#[cfg(test)]
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_its_shapes() {
        let size = 64;
        let img = render(size);
        let pixel = |x: f32, y: f32| {
            let i = (((y * size as f32) as u32 * size + (x * size as f32) as u32) * 4) as usize;
            [img[i], img[i + 1], img[i + 2], img[i + 3]]
        };
        assert_eq!(pixel(0.01, 0.01)[3], 0); // esquina: transparente (fuera del cuadrado redondeado)
        let (sx, sy) = orbit_point(STAR_ANGLE);
        let star = pixel(sx, sy);
        assert!(
            star[0] > 240 && star[2] < 200,
            "la estrella es dorada: {star:?}"
        );
        let (lx, ly) = orbit_point(PI); // lado izquierdo de la órbita
        let trail = pixel(lx, ly);
        assert!(
            trail[2] > trail[0] + 40,
            "la órbita es azul/violeta: {trail:?}"
        );

        // El .ico: cabecera con la cantidad de imágenes y el primer tamaño.
        let ico = to_ico(&[16, 256]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(ico[6], 16);
        assert_eq!(ico[6 + 16], 0); // 256 se escribe como 0
    }

    /// Regenera assets/orion.ico: `cargo test write_icon -- --ignored`.
    #[test]
    #[ignore]
    fn write_icon() {
        let ico = to_ico(&[16, 20, 24, 32, 40, 48, 64, 256]);
        std::fs::create_dir_all("assets").unwrap();
        std::fs::write("assets/orion.ico", ico).unwrap();
        // Vista previa en grande (RGBA crudo de 512×512) para mirarlo sin abrir la app.
        std::fs::write("target/icon-512.rgba", render(512)).unwrap();
        // El PNG del README (assets/orion.png) se arma desde ese archivo con scripts/icon-png.ps1.
    }

    #[test]
    fn png_is_well_formed() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082); // el CRC conocido de un IEND vacío
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        let png = to_png(4);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    /// Arma target/orion.iconset (los PNG que pide `iconutil` para el .icns de macOS):
    /// `cargo test write_iconset -- --ignored`. Lo usa scripts/package-macos.sh.
    #[test]
    #[ignore]
    fn write_iconset() {
        let dir = std::path::Path::new("target/orion.iconset");
        std::fs::create_dir_all(dir).unwrap();
        for size in [16, 32, 128, 256, 512] {
            std::fs::write(dir.join(format!("icon_{size}x{size}.png")), to_png(size)).unwrap();
            let retina = format!("icon_{size}x{size}@2x.png");
            std::fs::write(dir.join(retina), to_png(size * 2)).unwrap();
        }
    }
}
