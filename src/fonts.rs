//! Fuentes: buscarlas en Windows y macOS, detectar las monoespaciadas y cargarlas en egui.

use crate::config::config_path;
use eframe::egui;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::path::PathBuf;

/// Un archivo de fuente y cuál de sus fuentes usar: las colecciones (.ttc) traen varias
/// en un mismo archivo (ej. Menlo.ttc: normal, negrita, cursiva…). En un .ttf/.otf es 0.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FontFile {
    pub(crate) path: PathBuf,
    pub(crate) index: u32,
}

impl From<PathBuf> for FontFile {
    fn from(path: PathBuf) -> Self {
        Self { path, index: 0 }
    }
}

/// Fuente por defecto: Cascadia Mono en Windows; en macOS SF Mono si está, si no Menlo
/// (Menlo viene siempre con macOS).
pub(crate) fn default_family() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        static FAMILY: std::sync::OnceLock<&str> = std::sync::OnceLock::new();
        FAMILY.get_or_init(|| {
            if find_font("SF Mono").is_some() {
                "SF Mono"
            } else {
                "Menlo"
            }
        })
    }
    #[cfg(not(target_os = "macos"))]
    "Cascadia Mono"
}

/// Respaldos para los caracteres que le faltan a la fuente elegida, en orden.
/// Windows: Segoe UI Symbol (flechas, figuras y símbolos que muchas fuentes no traen).
/// macOS: Apple Symbols para lo mismo y Apple Color Emoji para los emoji a color.
#[cfg(target_os = "macos")]
const FALLBACK_FONTS: &[&str] = &["Apple Symbols", "Apple Color Emoji"];
#[cfg(not(target_os = "macos"))]
const FALLBACK_FONTS: &[&str] = &["Segoe UI Symbol"];

/// Familia de fuente para el texto en negrita.
pub(crate) fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

/// ¿Esta entrada del registro de fuentes es de `family`? Some(false) = normal, Some(true) = negrita.
/// Las entradas se ven como "Monocraft (TrueType)", "Cascadia Mono Regular (TrueType)"
/// o, con Windows en español, "Consolas Negrita (TrueType)".
pub(crate) fn face_weight(entry: &str, family: &str) -> Option<bool> {
    let face = entry.rsplit_once(" (").map_or(entry, |(face, _)| face);
    let face = face.to_lowercase();
    let style = face.strip_prefix(&family.to_lowercase())?.trim();
    match style {
        "" | "regular" | "normal" => Some(false),
        "bold" | "negrita" => Some(true),
        _ => None, // otro estilo (cursiva, light…) u otra fuente que empieza igual
    }
}

/// Todas las fuentes instaladas: (entrada del registro, archivo).
/// HKCU = instaladas solo para tu usuario (guardan la ruta completa).
/// HKLM = del sistema (guardan solo el nombre del archivo, dentro de C:\Windows\Fonts).
#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn registry_fonts() -> Vec<(String, FontFile)> {
    Vec::new()
}

/// Todas las fuentes instaladas: (entrada como "Menlo Bold", archivo).
/// macOS no tiene registro: se recorren las carpetas de fuentes (primero las de tu usuario,
/// como HKCU en Windows) y el nombre de cada una sale de la tabla `name` del archivo.
#[cfg(target_os = "macos")]
pub(crate) fn registry_fonts() -> Vec<(String, FontFile)> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    // Recorrer ~800 fuentes tarda unos 45 ms y al abrir se busca varias veces seguidas
    // (la elegida, los respaldos…): se reusa el último recorrido si es de hace muy poco.
    // Así, al abrir el menú más tarde, aparecen las fuentes recién instaladas.
    type Fonts = Vec<(String, FontFile)>;
    static LAST: Mutex<Option<(Instant, Fonts)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((when, fonts)) = last.as_ref()
        && when.elapsed() < Duration::from_secs(2)
    {
        return fonts.clone();
    }
    let fonts = scan_font_dirs();
    *last = Some((Instant::now(), fonts.clone()));
    fonts
}

#[cfg(target_os = "macos")]
fn scan_font_dirs() -> Vec<(String, FontFile)> {
    let home = std::env::var_os("HOME").map(|home| Path::new(&home).join("Library/Fonts"));
    let mut dirs: Vec<PathBuf> = home.into_iter().collect();
    dirs.extend(["/Library/Fonts", "/System/Library/Fonts"].map(PathBuf::from));
    dirs.reverse(); // se sacan del final: la del usuario queda primero
    let mut fonts = Vec::new();
    // Con subcarpetas: /System/Library/Fonts/Supplemental trae Arial, Courier New, Andale Mono…
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<_> = entries.flatten().collect();
        files.sort_by_key(|e| e.file_name());
        let mut subdirs = Vec::new();
        for entry in files {
            let path = entry.path();
            // `file_type` no sigue enlaces simbólicos: una carpeta enlazada no puede dar vueltas.
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                subdirs.push(path);
                continue;
            }
            let lower = path.to_string_lossy().to_lowercase();
            if !(is_font_file(&lower) || lower.ends_with(".ttc")) {
                continue; // .dfont y otros formatos viejos de Mac: egui no los lee
            }
            let Ok(mut opened) = std::fs::File::open(&path) else {
                continue;
            };
            for index in 0..face_count(&path) {
                if !has_outlines(&mut opened, index) {
                    continue; // solo mapas de bits: egui no puede usarla, ni la ofrecemos
                }
                // Las que empiezan con punto (".SF NS Mono") son privadas del sistema:
                // macOS las esconde de los menús de fuentes, y nosotros también.
                if let Some((family, style)) = face_names(&path, index)
                    && !family.starts_with('.')
                {
                    let file = FontFile {
                        path: path.clone(),
                        index,
                    };
                    fonts.push((format!("{family} {style}"), file));
                }
            }
        }
        dirs.extend(subdirs.into_iter().rev());
    }
    fonts
}

#[cfg(windows)]
pub(crate) fn registry_fonts() -> Vec<(String, FontFile)> {
    use winreg::{RegKey, enums::*};
    const FONTS_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";
    let windows = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    let system_fonts = Path::new(&windows).join("Fonts");
    let mut fonts = Vec::new();
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let Ok(key) = RegKey::predef(root).open_subkey(FONTS_KEY) else {
            continue;
        };
        for (entry, _) in key.enum_values().flatten() {
            if let Ok(file) = key.get_value::<String, _>(&entry) {
                // Las .fon (mapas de bits de Windows 3.1, ej. "Courier 10,12,15") no son TrueType/OpenType:
                // egui no puede usarlas, así que ni las ofrecemos.
                let lower = file.to_lowercase();
                if !(is_font_file(&lower) || lower.ends_with(".ttc")) {
                    continue;
                }
                // `join` con una ruta absoluta la deja tal cual, así que sirve para ambos casos.
                fonts.push((entry, system_fonts.join(file).into()));
            }
        }
    }
    fonts
}

/// Lee un archivo de fuente solo si de verdad es TrueType/OpenType: egui *cierra el programa*
/// (panic) si le pasan otra cosa, en vez de devolver un error. Los primeros 4 bytes lo dicen,
/// y además la fuente elegida tiene que traer contornos (ver `has_outlines`).
fn read_font(file: &FontFile) -> Option<Vec<u8>> {
    let bytes = std::fs::read(&file.path).ok()?;
    let magic: [&[u8]; 4] = [&[0, 1, 0, 0], b"OTTO", b"true", b"ttcf"];
    let outlines = || has_outlines(&mut std::io::Cursor::new(&bytes), file.index);
    if magic.iter().any(|m| bytes.starts_with(m)) && outlines() {
        Some(bytes)
    } else {
        eprintln!(
            "orion: {} no es una fuente TrueType/OpenType",
            file.path.display()
        );
        None
    }
}

/// ¿La fuente trae contornos que egui pueda dibujar? Algunas de macOS son solo mapas de bits
/// (ej. "GB18030 Bitmap": tablas `bhed`/`bdat`, sin `head`) y hacían caer a egui, como las .fon.
fn has_outlines(file: &mut (impl Read + Seek), index: u32) -> bool {
    let mut has =
        |tag| file.rewind().is_ok() && find_table(file, index, tag).is_ok_and(|t| t.is_some());
    has(b"head") && (has(b"glyf") || has(b"CFF ") || has(b"CFF2"))
}

pub(crate) fn is_font_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".ttf") || lower.ends_with(".otf")
}

/// Archivos (normal, negrita si existe) de una fuente: una ruta directa o el nombre de una fuente instalada.
pub(crate) fn find_font(family: &str) -> Option<(FontFile, Option<FontFile>)> {
    if is_font_file(family) {
        return Some((PathBuf::from(family).into(), None));
    }
    let (mut regular, mut bold) = (None, None);
    for (entry, file) in registry_fonts() {
        if let Some(is_bold) = face_weight(&entry, family) {
            let slot = if is_bold { &mut bold } else { &mut regular };
            slot.get_or_insert(file);
        }
    }
    regular.map(|regular| (regular, bold))
}

fn read_u16(file: &mut impl Read) -> std::io::Result<u16> {
    let mut bytes = [0u8; 2];
    file.read_exact(&mut bytes)?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u32(file: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0u8; 4];
    file.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

/// Cuántas fuentes trae el archivo: las colecciones (.ttc) dicen cuántas en su cabecera
/// ("ttcf" + versión + cantidad); un .ttf/.otf trae una.
#[cfg(target_os = "macos")] // solo lo usa el recorrido de carpetas de macOS
fn face_count(path: &Path) -> u32 {
    let read = || -> std::io::Result<u32> {
        let mut file = std::fs::File::open(path)?;
        if read_u32(&mut file)?.to_be_bytes() != *b"ttcf" {
            return Ok(1);
        }
        file.seek(SeekFrom::Start(8))?;
        read_u32(&mut file)
    };
    // Un tope por si el archivo está roto: ninguna colección real trae tantas.
    read().unwrap_or(0).min(256)
}

/// Busca la tabla `tag` de la fuente `index` y devuelve dónde empieza y cuánto mide.
/// Un archivo de fuente empieza con un índice de tablas (nombre de 4 letras + posición),
/// así que basta leer unos pocos bytes, no el archivo entero. En una colección (.ttc),
/// la cabecera dice dónde empieza el índice de cada fuente.
fn find_table(
    file: &mut (impl Read + Seek),
    index: u32,
    tag: &[u8; 4],
) -> std::io::Result<Option<(u64, u32)>> {
    let mut start = 0;
    if read_u32(file)?.to_be_bytes() == *b"ttcf" {
        // "ttcf" + versión (4) + cantidad (4) + la posición (4) de cada fuente.
        file.seek(SeekFrom::Start(12 + 4 * index as u64))?;
        start = read_u32(file)? as u64;
    }
    file.seek(SeekFrom::Start(start + 4))?;
    let tables = read_u16(file)? as usize;
    file.seek(SeekFrom::Start(start + 12))?;
    let mut directory = vec![0u8; tables * 16];
    file.read_exact(&mut directory)?;
    // Cada entrada: tag (4) + checksum (4) + offset (4) + largo (4).
    // as_chunks da arreglos de tamaño fijo ([u8; 16]): el compilador sabe que los índices son válidos.
    for entry in directory.as_chunks::<16>().0 {
        if &entry[..4] == tag {
            let offset = u32::from_be_bytes([entry[8], entry[9], entry[10], entry[11]]);
            let len = u32::from_be_bytes([entry[12], entry[13], entry[14], entry[15]]);
            return Ok(Some((offset as u64, len)));
        }
    }
    Ok(None)
}

/// ¿La fuente es monoespaciada? Lo dice el flag `isFixedPitch` de su tabla `post`.
pub(crate) fn is_monospace(path: &Path, index: u32) -> bool {
    let read = || -> std::io::Result<bool> {
        let mut file = std::fs::File::open(path)?;
        let Some((post, _)) = find_table(&mut file, index, b"post")? else {
            return Ok(false);
        };
        // isFixedPitch es un u32 en el byte 12 de la tabla post: distinto de 0 = monoespaciada.
        file.seek(SeekFrom::Start(post + 12))?;
        Ok(read_u32(&mut file)? != 0)
    };
    read().unwrap_or(false)
}

/// (familia, estilo) de la fuente `index`, en inglés, leídos de su tabla `name`:
/// ("Menlo", "Bold"), ("SF Mono", "Regular").
#[cfg(target_os = "macos")] // solo lo usa el recorrido de carpetas de macOS
fn face_names(path: &Path, index: u32) -> Option<(String, String)> {
    let mut file = std::fs::File::open(path).ok()?;
    let (offset, len) = find_table(&mut file, index, b"name").ok()??;
    // `len` viene del archivo: con un tope, un archivo roto no pide gigas de memoria.
    if len > 1 << 20 {
        return None;
    }
    let mut table = vec![0u8; len as usize];
    file.seek(SeekFrom::Start(offset)).ok()?;
    file.read_exact(&mut table).ok()?;
    parse_names(&table)
}

/// Lee la tabla `name`: cabecera (formato, cantidad, dónde empiezan los textos) y después
/// un registro de 12 bytes por texto (plataforma, codificación, idioma, id, largo, posición).
#[cfg(target_os = "macos")] // solo lo usa el recorrido de carpetas de macOS
fn parse_names(table: &[u8]) -> Option<(String, String)> {
    let u16_at = |at: usize| Some(u16::from_be_bytes(table.get(at..at + 2)?.try_into().ok()?));
    let count = u16_at(2)? as usize;
    let strings = u16_at(4)? as usize;
    // Por cada id, el mejor texto encontrado y qué tan bueno es.
    let mut best: [Option<(u8, String)>; 4] = Default::default();
    for record in 0..count {
        let at = 6 + record * 12;
        let [platform, encoding, language, id, len, pos] =
            [0, 2, 4, 6, 8, 10].map(|i| u16_at(at + i).unwrap_or(u16::MAX));
        // 1 = familia, 2 = estilo; 16 y 17 = lo mismo, pero "tipográficos": en "Avenir Next
        // Condensed Heavy" el 1 dice toda la frase y el 2 "Regular", el 16 y el 17 la separan bien.
        let Some(slot) = [1, 2, 16, 17].iter().position(|&x| x == id) else {
            continue;
        };
        let start = strings + pos as usize;
        let Some(bytes) = table.get(start..start + len as usize) else {
            continue;
        };
        // Preferimos Windows en inglés de EE.UU. (UTF-16), después Unicode y al final Mac Roman.
        let (score, text) = match (platform, encoding, language) {
            (3, 0 | 1 | 10, lang) => (if lang == 0x409 { 3 } else { 2 }, utf16(bytes)),
            (0, _, _) => (2, utf16(bytes)),
            // Mac Roman en inglés: para nombres de fuentes, en la práctica ASCII.
            (1, 0, 0) => (1, bytes.iter().map(|&b| b as char).collect()),
            _ => continue,
        };
        let text = text.trim().to_string();
        if !text.is_empty() && best[slot].as_ref().is_none_or(|(s, _)| score > *s) {
            best[slot] = Some((score, text));
        }
    }
    let [family, style, typo_family, typo_style] = best.map(|b| b.map(|(_, text)| text));
    let family = typo_family.or(family)?;
    let style = typo_style.or(style).unwrap_or_else(|| "Regular".into());
    Some((family, style))
}

#[cfg(target_os = "macos")] // solo lo usa el recorrido de carpetas de macOS
fn utf16(bytes: &[u8]) -> String {
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c));
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Nombre de familia de una entrada del registro si es la variante normal, o None si es otro estilo.
/// "Cascadia Mono Regular (TrueType)" → "Cascadia Mono"; "Consolas Negrita (TrueType)" → None.
pub(crate) fn family_name(entry: &str) -> Option<String> {
    let face = entry.rsplit_once(" (").map_or(entry, |(face, _)| face);
    let family = face
        .strip_suffix(" Regular")
        .or_else(|| face.strip_suffix(" Normal"))
        .unwrap_or(face);
    // Si la última palabra es un estilo, es una variante (Bold, Cursiva…), no la versión normal.
    const STYLES: [&str; 17] = [
        "bold",
        "negrita",
        "italic",
        "cursiva",
        "inclinada",
        "oblique",
        "light",
        "semilight",
        "extralight",
        "thin",
        "medium",
        "semibold",
        "seminegrita",
        "black",
        "heavy",
        "extrabold",
        "condensed",
    ];
    // Algunas fuentes separan el estilo con guion: "Unispace-Bold".
    let last_word = family
        .rsplit([' ', '-'])
        .next()
        .unwrap_or_default()
        .to_lowercase();
    (!STYLES.contains(&last_word.as_str())).then(|| family.to_string())
}

/// Carpeta donde se copian las fuentes que agregas arrastrándolas: %APPDATA%\orion\fonts
pub(crate) fn fonts_dir() -> Option<PathBuf> {
    Some(config_path()?.parent()?.join("fonts"))
}

/// Una opción del menú de fuentes.
#[derive(Clone)]
pub(crate) struct FontChoice {
    /// Lo que se muestra en la lista.
    pub(crate) label: String,
    /// Lo que se guarda en `font.family`: un nombre instalado o una ruta.
    pub(crate) family: String,
    pub(crate) monospace: bool,
}

/// Fuentes para el menú: las instaladas en el sistema y las agregadas a Orion, ordenadas y sin repetir.
pub(crate) fn font_choices() -> Vec<FontChoice> {
    let mut fonts: Vec<FontChoice> = registry_fonts()
        .into_iter()
        .filter_map(|(entry, file)| {
            let name = family_name(&entry)?;
            Some(FontChoice {
                label: name.clone(),
                family: name,
                monospace: is_monospace(&file.path, file.index),
            })
        })
        .collect();
    let added = fonts_dir().and_then(|dir| std::fs::read_dir(dir).ok());
    for file in added.into_iter().flatten().flatten() {
        let path = file.path();
        let family = path.display().to_string();
        if is_font_file(&family) {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            fonts.push(FontChoice {
                label: format!("{stem} (agregada)"),
                family,
                monospace: is_monospace(&path, 0),
            });
        }
    }
    fonts.sort_by_key(|f| f.label.to_lowercase());
    fonts.dedup_by(|a, b| a.label == b.label);
    fonts
}

/// Copia una fuente a la carpeta de Orion y devuelve la ruta nueva (lo que va en `font.family`).
pub(crate) fn add_font(source: &Path) -> std::io::Result<PathBuf> {
    let dir = fonts_dir().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::create_dir_all(&dir)?;
    let name = source.file_name().ok_or(std::io::ErrorKind::InvalidInput)?;
    let dest = dir.join(name);
    if dest != source {
        std::fs::copy(source, &dest)?;
    }
    Ok(dest)
}

/// Busca la fuente, la carga en egui y avisa en la consola cuál quedó.
pub(crate) fn apply_font(ctx: &egui::Context, family: &str) {
    let files = find_font(family).or_else(|| {
        let default = default_family();
        eprintln!("orion: no encontré la fuente \"{family}\"; uso {default}");
        find_font(default)
    });
    match &files {
        Some((regular, bold)) => {
            let bold = bold.as_ref().map(|b| (b.path.display(), b.index));
            let (path, index) = (regular.path.display(), regular.index);
            eprintln!("orion: fuente {path} #{index} (negrita: {bold:?})")
        }
        None => eprintln!("orion: sin fuente propia, uso la de egui"),
    }
    ctx.set_fonts(font_definitions(files.as_ref()));
}

/// Fuentes de la terminal: la elegida (normal y negrita) y, detrás, los respaldos.
/// Si a la fuente le falta un carácter, egui lo busca en la siguiente de la lista.
pub(crate) fn font_definitions(
    files: Option<&(FontFile, Option<FontFile>)>,
) -> egui::FontDefinitions {
    use egui::{FontData, FontFamily, FontTweak, epaint::text::VariationCoords};
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    // Respaldos que ya trae egui (incluyen emoji en blanco y negro).
    let mut fallback = fonts.families[&FontFamily::Monospace].clone();
    // Y los del sistema (ver FALLBACK_FONTS), si están instalados. Se leen una sola vez:
    // Apple Color Emoji pesa ~190 MB y no vale releerla cada vez que se cambia de fuente.
    static SYSTEM_FALLBACKS: std::sync::OnceLock<Vec<(String, Arc<FontData>)>> =
        std::sync::OnceLock::new();
    let system = SYSTEM_FALLBACKS.get_or_init(|| {
        let load = |name: &&str| {
            let (file, _) = find_font(name)?;
            let mut data = FontData::from_owned(read_font(&file)?);
            data.index = file.index;
            Some((name.to_string(), Arc::new(data)))
        };
        FALLBACK_FONTS.iter().filter_map(load).collect()
    });
    for (name, data) in system {
        fonts.font_data.insert(name.clone(), data.clone());
        fallback.push(name.clone());
    }

    let mut mono = fallback.clone();
    let mut bold = fallback;
    if let Some((regular_file, bold_file)) = files
        && let Some(bytes) = read_font(regular_file)
    {
        // "wght" es el eje de peso de las fuentes variables (400 normal, 700 negrita).
        // En fuentes no variables se ignora.
        let font = |bytes: Vec<u8>, index: u32, wght: f32| {
            let tweak = FontTweak {
                coords: VariationCoords::new([(b"wght", wght)]),
                ..Default::default()
            };
            let mut data = FontData::from_owned(bytes).tweak(tweak);
            data.index = index;
            Arc::new(data)
        };
        // Si hay archivo "Bold" aparte, se usa; si no, el mismo archivo con peso 700
        // (una fuente variable como Cascadia engorda; una fija se queda igual).
        let bold_data = match bold_file
            .as_ref()
            .and_then(|f| Some((read_font(f)?, f.index)))
        {
            Some((bold_bytes, index)) => font(bold_bytes, index, 700.0),
            None => font(bytes.clone(), regular_file.index, 700.0),
        };
        fonts
            .font_data
            .insert("mono".into(), font(bytes, regular_file.index, 400.0));
        fonts.font_data.insert("mono-bold".into(), bold_data);
        mono.insert(0, "mono".into());
        bold.insert(0, "mono-bold".into());
    }
    // Sin fuente propia, ambas familias quedan con la de egui (pero existen: egui falla si falta una).
    fonts.families.insert(FontFamily::Monospace, mono);
    fonts.families.insert(bold_family(), bold);
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_put_chosen_font_first_with_fallbacks() {
        let files = find_font("Cascadia Mono");
        let fonts = font_definitions(files.as_ref());
        let mono = &fonts.families[&egui::FontFamily::Monospace];
        let bold = &fonts.families[&bold_family()];
        if let Some((regular, _)) = &files {
            assert!(
                regular.path.exists(),
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
        assert_eq!(direct.0, PathBuf::from(r"C:\x\Mi Fuente.otf").into());
    }

    #[test]
    fn registry_entries_give_family_names() {
        assert_eq!(
            family_name("Monocraft (TrueType)").as_deref(),
            Some("Monocraft")
        );
        assert_eq!(
            family_name("Cascadia Mono Regular (TrueType)").as_deref(),
            Some("Cascadia Mono")
        );
        assert_eq!(family_name("Consolas Negrita (TrueType)"), None); // variante, no familia
        assert_eq!(family_name("Arial Bold Italic (TrueType)"), None);
        assert_eq!(
            family_name("Lucida Sans Typewriter Inclinada (TrueType)"),
            None
        );
        assert_eq!(family_name("Unispace-Bold (TrueType)"), None);
    }

    #[test]
    fn detects_monospace_fonts() {
        let fonts = Path::new(r"C:\Windows\Fonts");
        // Solo las que existan: Windows Server (el runner de CI) no trae todas las de Windows 11.
        for (file, mono) in [
            ("consola.ttf", true), // Consolas
            ("CascadiaMono.ttf", true),
            ("cour.ttf", true), // Courier New
            ("arial.ttf", false),
        ] {
            let path = fonts.join(file);
            if path.exists() {
                assert_eq!(is_monospace(&path, 0), mono, "{file}");
            }
        }
        assert!(!is_monospace(&fonts.join("no-existe.ttf"), 0));
    }
}

#[cfg(test)]
mod font_loading {
    use super::*;

    /// Carga los archivos en egui y dibuja texto, como al elegir una fuente en el menú.
    /// Devuelve false si egui se cayó.
    fn loads(files: Option<(FontFile, Option<FontFile>)>) -> bool {
        std::panic::catch_unwind(|| {
            let ctx = egui::Context::default();
            ctx.set_fonts(font_definitions(files.as_ref()));
            for _ in 0..2 {
                // Como la app: medir la celda con la fuente monoespaciada y dibujar texto.
                let mut out = ctx.run_ui(Default::default(), |ui| {
                    let font = egui::FontId::monospace(14.0);
                    let bold = egui::FontId::new(14.0, bold_family());
                    ui.ctx()
                        .fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
                    ui.label(egui::RichText::new("Mg─│é→ 123").font(font));
                    ui.label(egui::RichText::new("negrita").font(bold));
                });
                out.textures_delta.clear(); // sin ventana, nadie sube las texturas
            }
        })
        .is_ok()
    }

    /// Las .fon de Windows (mapas de bits, ej. "Courier 10,12,15") hacían caer a Orion al elegirlas.
    #[test]
    fn a_bitmap_font_does_not_crash() {
        let fon = Path::new(r"C:\Windows\Fonts\coure.fon");
        if fon.exists() {
            assert!(
                loads(Some((fon.to_path_buf().into(), None))),
                "Orion se cayó con {fon:?}"
            );
        }
        // Y ya no aparecen en la lista del menú.
        let listed = registry_fonts();
        assert!(listed.iter().all(|(_, file)| {
            let ext = file
                .path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            ext != "fon"
        }));
    }

    /// Todas las fuentes de la lista cargan sin caerse: `cargo test every_font -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn every_font_choice_loads() {
        let broken: Vec<String> = font_choices()
            .into_iter()
            .filter(|choice| !loads(find_font(&choice.family)))
            .map(|choice| choice.family)
            .collect();
        assert!(broken.is_empty(), "se caen: {broken:?}");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos {
    use super::*;

    #[test]
    fn menlo_comes_from_its_collection_with_bold() {
        let (regular, bold) = find_font("Menlo").expect("Menlo viene con macOS");
        let bold = bold.expect("Menlo trae negrita");
        assert!(regular.path.ends_with("Menlo.ttc"));
        assert_eq!(regular.path, bold.path);
        assert_ne!(regular.index, bold.index, "son fuentes distintas del .ttc");
        assert_eq!(
            face_names(&regular.path, regular.index).unwrap().1,
            "Regular"
        );
        assert_eq!(face_names(&bold.path, bold.index).unwrap().1, "Bold");
    }

    #[test]
    fn names_come_from_the_name_table() {
        let fonts = Path::new("/System/Library/Fonts");
        assert_eq!(
            face_names(&fonts.join("Monaco.ttf"), 0),
            Some(("Monaco".into(), "Regular".into()))
        );
        assert_eq!(face_count(&fonts.join("Monaco.ttf")), 1);
        assert!(face_count(&fonts.join("Menlo.ttc")) > 1);
        assert_eq!(face_count(&fonts.join("no-existe.ttc")), 0);
        assert_eq!(face_names(&fonts.join("no-existe.ttf"), 0), None);
    }

    /// "GB18030 Bitmap" (solo mapas de bits) hacía caer a egui al elegirla en el menú.
    #[test]
    fn a_bitmap_only_font_is_not_offered_nor_loaded() {
        let path = PathBuf::from("/System/Library/Fonts/Supplemental/NISC18030.ttf");
        if path.exists() {
            assert_eq!(read_font(&path.clone().into()), None);
            assert!(registry_fonts().iter().all(|(_, file)| file.path != path));
        }
        let menlo = find_font("Menlo").unwrap().0;
        assert!(read_font(&menlo).is_some());
    }

    #[test]
    fn default_font_is_sf_mono_or_menlo() {
        let default = default_family();
        let expected = if find_font("SF Mono").is_some() {
            "SF Mono"
        } else {
            "Menlo"
        };
        assert_eq!(default, expected);
        assert!(find_font(default).is_some());
    }

    #[test]
    fn menu_lists_macos_fonts_with_monospace_flag() {
        let choices = font_choices();
        let find = |name: &str| choices.iter().find(|c| c.family == name);
        assert!(find("Menlo").expect("Menlo en el menú").monospace);
        // Viene en Supplemental, una subcarpeta de /System/Library/Fonts.
        assert!(
            find("Courier New")
                .expect("Courier New en el menú")
                .monospace
        );
        // Monaco es de ancho fijo, pero su tabla `post` dice que no: se lista sin marca.
        assert!(find("Monaco").is_some());
        assert!(!find("Helvetica").expect("Helvetica en el menú").monospace);
        // Las variantes no aparecen como familias.
        assert!(find("Menlo Bold").is_none());
        // Ni las privadas del sistema.
        assert!(choices.iter().all(|c| !c.family.starts_with('.')));
    }

    #[test]
    fn fallbacks_are_macos_fonts() {
        let fonts = font_definitions(None);
        let mono = &fonts.families[&egui::FontFamily::Monospace];
        for name in ["Apple Symbols", "Apple Color Emoji"] {
            assert!(mono.iter().any(|f| f == name), "falta el respaldo {name}");
        }
        assert!(!mono.iter().any(|f| f == "Segoe UI Symbol"));
    }

    #[test]
    fn collection_fonts_load_in_egui() {
        // Menlo y los respaldos vienen en .ttc: egui los carga con el índice correcto.
        let ok = std::panic::catch_unwind(|| {
            let ctx = egui::Context::default();
            ctx.set_fonts(font_definitions(find_font("Menlo").as_ref()));
            let mut out = ctx.run_ui(Default::default(), |ui| {
                let font = egui::FontId::monospace(14.0);
                ui.label(egui::RichText::new("Mg─│→ ⌘ 😀").font(font));
            });
            out.textures_delta.clear();
        });
        assert!(ok.is_ok());
    }
}
