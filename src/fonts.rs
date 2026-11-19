//! Fuentes: buscarlas en Windows, detectar las monoespaciadas y cargarlas en egui.

use crate::config::config_path;
use eframe::egui;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

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
pub(crate) fn registry_fonts() -> Vec<(String, PathBuf)> {
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
                fonts.push((entry, system_fonts.join(file)));
            }
        }
    }
    fonts
}

/// Lee un archivo de fuente solo si de verdad es TrueType/OpenType: egui *cierra el programa*
/// (panic) si le pasan otra cosa, en vez de devolver un error. Los primeros 4 bytes lo dicen.
fn read_font(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    let magic: [&[u8]; 4] = [&[0, 1, 0, 0], b"OTTO", b"true", b"ttcf"];
    if magic.iter().any(|m| bytes.starts_with(m)) {
        Some(bytes)
    } else {
        eprintln!(
            "orion: {} no es una fuente TrueType/OpenType",
            path.display()
        );
        None
    }
}

pub(crate) fn is_font_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".ttf") || lower.ends_with(".otf")
}

/// Archivos (normal, negrita si existe) de una fuente: una ruta directa o el nombre de una fuente instalada.
pub(crate) fn find_font(family: &str) -> Option<(PathBuf, Option<PathBuf>)> {
    if is_font_file(family) {
        return Some((family.into(), None));
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

/// ¿La fuente es monoespaciada? Lo dice el flag `isFixedPitch` de su tabla `post`.
/// Un archivo de fuente empieza con un índice de tablas (nombre de 4 letras + posición),
/// así que basta leer unos pocos bytes, no el archivo entero.
pub(crate) fn is_monospace(path: &Path) -> bool {
    use std::io::{Seek, SeekFrom};
    let read = || -> std::io::Result<bool> {
        let mut file = std::fs::File::open(path)?;
        let mut header = [0u8; 12];
        file.read_exact(&mut header)?;
        // ponytail: las colecciones .ttc (varias fuentes en un archivo) se tratan como no monoespaciadas.
        if &header[..4] == b"ttcf" {
            return Ok(false);
        }
        let tables = u16::from_be_bytes([header[4], header[5]]) as usize;
        let mut directory = vec![0u8; tables * 16];
        file.read_exact(&mut directory)?;
        // Cada entrada: tag (4) + checksum (4) + offset (4) + largo (4).
        // as_chunks da arreglos de tamaño fijo ([u8; 16]): el compilador sabe que los índices son válidos.
        for entry in directory.as_chunks::<16>().0 {
            if &entry[..4] == b"post" {
                let offset = u32::from_be_bytes([entry[8], entry[9], entry[10], entry[11]]);
                // isFixedPitch es un u32 en el byte 12 de la tabla post: distinto de 0 = monoespaciada.
                file.seek(SeekFrom::Start(offset as u64 + 12))?;
                let mut flag = [0u8; 4];
                file.read_exact(&mut flag)?;
                return Ok(flag != [0; 4]);
            }
        }
        Ok(false)
    };
    read().unwrap_or(false)
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

/// Fuentes para el menú: las instaladas en Windows y las agregadas a Orion, ordenadas y sin repetir.
pub(crate) fn font_choices() -> Vec<FontChoice> {
    let mut fonts: Vec<FontChoice> = registry_fonts()
        .into_iter()
        .filter_map(|(entry, file)| {
            let name = family_name(&entry)?;
            Some(FontChoice {
                label: name.clone(),
                family: name,
                monospace: is_monospace(&file),
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
                monospace: is_monospace(&path),
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
        eprintln!("orion: no encontré la fuente \"{family}\"; uso Cascadia Mono");
        find_font("Cascadia Mono")
    });
    match &files {
        Some((regular, bold)) => {
            eprintln!("orion: fuente {} (negrita: {bold:?})", regular.display())
        }
        None => eprintln!("orion: sin fuente propia, uso la de egui"),
    }
    ctx.set_fonts(font_definitions(files.as_ref()));
}

/// Fuentes de la terminal: la elegida (normal y negrita) y, detrás, los respaldos.
/// Si a la fuente le falta un carácter, egui lo busca en la siguiente de la lista.
pub(crate) fn font_definitions(
    files: Option<&(PathBuf, Option<PathBuf>)>,
) -> egui::FontDefinitions {
    use egui::{FontData, FontFamily, FontTweak, epaint::text::VariationCoords};
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    // Respaldos que ya trae egui (incluyen emoji en blanco y negro).
    let mut fallback = fonts.families[&FontFamily::Monospace].clone();
    // Segoe UI Symbol (viene con Windows): flechas, figuras y símbolos que muchas fuentes no traen.
    if let Some((symbols, _)) = find_font("Segoe UI Symbol")
        && let Some(bytes) = read_font(&symbols)
    {
        let data = Arc::new(FontData::from_owned(bytes));
        fonts.font_data.insert("symbols".into(), data);
        fallback.push("symbols".into());
    }

    let mut mono = fallback.clone();
    let mut bold = fallback;
    if let Some((regular_file, bold_file)) = files
        && let Some(bytes) = read_font(regular_file)
    {
        // "wght" es el eje de peso de las fuentes variables (400 normal, 700 negrita).
        // En fuentes no variables se ignora.
        let font = |bytes: Vec<u8>, wght: f32| {
            let tweak = FontTweak {
                coords: VariationCoords::new([(b"wght", wght)]),
                ..Default::default()
            };
            Arc::new(FontData::from_owned(bytes).tweak(tweak))
        };
        // Si hay archivo "Bold" aparte, se usa; si no, el mismo archivo con peso 700
        // (una fuente variable como Cascadia engorda; una fija se queda igual).
        let bold_bytes = bold_file.as_ref().and_then(|f| read_font(f));
        let bold_data = font(bold_bytes.unwrap_or_else(|| bytes.clone()), 700.0);
        fonts.font_data.insert("mono".into(), font(bytes, 400.0));
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
                regular.exists(),
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
        assert_eq!(direct.0, PathBuf::from(r"C:\x\Mi Fuente.otf"));
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
                assert_eq!(is_monospace(&path), mono, "{file}");
            }
        }
        assert!(!is_monospace(&fonts.join("no-existe.ttf")));
    }
}

#[cfg(test)]
mod font_loading {
    use super::*;

    /// Carga los archivos en egui y dibuja texto, como al elegir una fuente en el menú.
    /// Devuelve false si egui se cayó.
    fn loads(files: Option<(PathBuf, Option<PathBuf>)>) -> bool {
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
            assert!(loads(Some((fon.into(), None))), "Orion se cayó con {fon:?}");
        }
        // Y ya no aparecen en la lista del menú.
        let listed = registry_fonts();
        assert!(listed.iter().all(|(_, file)| {
            let ext = file
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
