//! El ícono de Orion, dibujado con código: la cara de un gato (Orion se llama por un gato) hecha
//! con rectas, que guiña: un ojo cerrado (^) y el otro es el `>` del prompt de una terminal.
//!
//! Cada figura es una "función de distancia" (SDF): para cada píxel, cuánto falta para llegar al borde
//! (negativo = adentro). Con eso el borde sale suave (antialiasing) en cualquier tamaño, de 16 a 1024 px.
//! Se usa para el ícono de la ventana, con `to_ico` para generar assets/orion.ico (el del .exe) y con
//! `to_png` para el .icns de Orion.app (lo arma scripts/package-macos.sh).

/// RGBA lineal entre 0 y 1, con el alfa sin premultiplicar.
type Rgba = [f32; 4];

fn hex(v: u32, a: f32) -> Rgba {
    let c = |s: u32| ((v >> s) & 0xff) as f32 / 255.0;
    [c(16), c(8), c(0), a]
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

/// Distancia a un polígono cerrado (negativa adentro): la más corta a sus lados, con el signo
/// según si el punto está adentro (regla par-impar).
fn polygon(x: f32, y: f32, points: &[(f32, f32)]) -> f32 {
    let mut dist = f32::MAX;
    let mut inside = false;
    for (i, &(ax, ay)) in points.iter().enumerate() {
        let (bx, by) = points[(i + 1) % points.len()];
        let (ex, ey) = (bx - ax, by - ay);
        let t = (((x - ax) * ex + (y - ay) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        dist = dist.min((x - ax - ex * t).hypot(y - ay - ey * t));
        if (ay > y) != (by > y) && x < ax + (y - ay) / ey * ex {
            inside = !inside;
        }
    }
    if inside { -dist } else { dist }
}

// Geometría, en las unidades del diseño: un cuadrado de 120 × 120 (y hacia abajo).
/// La cara: orejas en punta, mejillas rectas y el mentón en punta.
const FACE: [(f32, f32); 9] = [
    (24.0, 50.0),
    (26.0, 16.0),
    (48.0, 34.0),
    (72.0, 34.0),
    (94.0, 16.0),
    (96.0, 50.0),
    (88.0, 80.0),
    (60.0, 100.0),
    (32.0, 80.0),
];
/// Ojo derecho, cerrado: una "^" de 20 × 10 con centro en (74, 59).
const CLOSED_EYE: [(f32, f32); 6] = [
    (64.0, 64.0),
    (74.0, 54.0),
    (84.0, 64.0),
    (80.0, 64.0),
    (74.0, 58.0),
    (68.0, 64.0),
];
/// Ojo izquierdo, el guiño: la misma "^" girada 90° (un ">"), con centro en (46, 59).
/// Mismo largo y mismo grosor que el otro ojo.
const PROMPT_EYE: [(f32, f32); 6] = [
    (41.0, 49.0),
    (51.0, 59.0),
    (41.0, 69.0),
    (41.0, 65.0),
    (47.0, 59.0),
    (41.0, 53.0),
];

/// Dónde va el diseño dentro del ícono (0..1): ocupa el cuadrado redondeado del fondo.
const MARGIN: f32 = 0.04;
const SCALE: f32 = (1.0 - 2.0 * MARGIN) / 120.0;

/// Color de un punto del ícono (x, y en 0..1). `px` = tamaño de un píxel, para el antialiasing.
fn color_at(x: f32, y: f32, px: f32) -> Rgba {
    let coverage = |d: f32, px: f32| (0.5 - d / px).clamp(0.0, 1.0);
    let mut c: Rgba = [0.0; 4];

    // Fondo: cuadrado redondeado oscuro, plano.
    let tile = rounded_square(x - 0.5, y - 0.5, 0.5 - MARGIN, 0.2);
    c = over(c, hex(0x0d1117, 1.0), coverage(tile, px));

    // La cara, con los ojos recortados (se ve el fondo a través de ellos).
    let (dx, dy, dpx) = ((x - MARGIN) / SCALE, (y - MARGIN) / SCALE, px / SCALE);
    let eyes = polygon(dx, dy, &CLOSED_EYE).min(polygon(dx, dy, &PROMPT_EYE));
    let face = polygon(dx, dy, &FACE).max(-eyes);
    c = over(c, hex(0xe6edf3, 1.0), coverage(face, dpx));
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
        let size = 120; // una unidad del diseño ≈ un píxel
        let img = render(size);
        // Punto del diseño (0..120) → píxel del ícono.
        let pixel = |x: f32, y: f32| {
            let to_px = |v: f32| ((MARGIN + v * SCALE) * size as f32) as u32;
            let i = ((to_px(y) * size + to_px(x)) * 4) as usize;
            [img[i], img[i + 1], img[i + 2], img[i + 3]]
        };
        let is_face = |p: [u8; 4]| p[0] > 200 && p[3] == 255;
        let is_background = |p: [u8; 4]| p[0] < 40 && p[3] == 255;
        assert_eq!(
            render(size)[3],
            0,
            "la esquina es transparente (fuera del cuadrado)"
        );
        assert!(is_face(pixel(60.0, 80.0)), "la cara es clara");
        assert!(
            is_background(pixel(60.0, 20.0)),
            "entre las orejas se ve el fondo"
        );
        assert!(
            is_background(pixel(74.0, 56.0)),
            "el ojo cerrado (^) es un recorte"
        );
        assert!(
            is_background(pixel(49.0, 59.0)),
            "el ojo de prompt (>) es un recorte"
        );
        assert!(
            is_face(pixel(43.0, 59.0)),
            "dentro de la abertura del > sigue la cara"
        );
        assert!(is_face(pixel(46.0, 66.0)), "debajo del > vuelve la cara");

        // Los dos ojos miden lo mismo: el > es la ^ girada.
        let span = |eye: &[(f32, f32)], axis: fn(&(f32, f32)) -> f32| {
            let values: Vec<f32> = eye.iter().map(axis).collect();
            let max = values.iter().cloned().fold(f32::MIN, f32::max);
            let min = values.iter().cloned().fold(f32::MAX, f32::min);
            max - min
        };
        let (w, h) = (|p: &(f32, f32)| p.0, |p: &(f32, f32)| p.1);
        assert_eq!(span(&CLOSED_EYE, w), span(&PROMPT_EYE, h));
        assert_eq!(span(&CLOSED_EYE, h), span(&PROMPT_EYE, w));

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
