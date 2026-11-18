//! Configuración del usuario: leer y guardar %APPDATA%\orion\config.toml.

use std::path::PathBuf;

// ponytail: se lee al arrancar y la escribe el menú (Ctrl+,); la recarga en caliente al editar a mano llega con #24.

/// Texto de config.toml. Se regenera completo al guardar desde el menú.
/// ponytail: los comentarios que agregues a mano se pierden al guardar; toml_edit los conservaría.
pub(crate) fn config_text(settings: &Settings) -> String {
    // toml::Value escribe el texto con las comillas y escapes correctos (ej. rutas con '\').
    let family = toml::Value::String(settings.font.family.clone());
    format!(
        r#"# Configuración de Orion. Se puede editar aquí o desde el menú (Ctrl+, o clic derecho).
# Si editas a mano, los cambios se aplican al reiniciar Orion.

[font]
# Nombre de una fuente instalada (igual que en VS Code), o la ruta a un archivo .ttf/.otf.
# Ejemplos: "Cascadia Mono", "Monocraft", 'C:\fuentes\MiFuente.ttf'
family = {family}
# Las fuentes pixeladas se ven nítidas en múltiplos de su grilla (Monocraft: 18, 27…).
size = {:.1}
# Alto de cada línea respecto de la letra: 1.0 = pegadas, 1.2 = con aire, 1.5 = muy espaciadas.
line_height = {:.2}
"#,
        settings.font.size, settings.font.line_height
    )
}

/// Guarda la configuración en config.toml (creando la carpeta si hace falta).
pub(crate) fn save_settings(settings: &Settings) -> std::io::Result<()> {
    let path = config_path().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::create_dir_all(path.parent().expect("la ruta tiene carpeta"))?;
    std::fs::write(path, config_text(settings))
}

#[derive(serde::Deserialize, Default, Clone, PartialEq)]
#[serde(default)] // lo que falte en el archivo toma el valor por defecto
pub(crate) struct Settings {
    pub(crate) font: FontSettings,
}

#[derive(serde::Deserialize, Clone, PartialEq)]
#[serde(default)]
pub(crate) struct FontSettings {
    pub(crate) family: String,
    pub(crate) size: f32,
    /// Interlineado: alto de la fila = alto de la letra × esto (la letra va centrada).
    pub(crate) line_height: f32,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            family: "Cascadia Mono".into(),
            size: 14.0,
            line_height: 1.2,
        }
    }
}

/// %APPDATA%\orion\config.toml
pub(crate) fn config_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("orion").join("config.toml"))
}

/// Lee la configuración. Si no existe la crea; si tiene errores, avisa y usa los valores por defecto
/// (una coma mal puesta no debería impedir abrir la terminal).
pub(crate) fn load_settings() -> Settings {
    let Some(path) = config_path() else {
        return Settings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
            eprintln!("orion: error en {}:\n{e}", path.display());
            Settings::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Primera vez: dejamos el archivo con los valores por defecto para que sea fácil encontrarlo.
            let settings = Settings::default();
            let _ = save_settings(&settings);
            settings
        }
        Err(e) => {
            eprintln!("orion: no pude leer {}: {e}", path.display());
            Settings::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parses_and_fills_defaults() {
        let s: Settings = toml::from_str(&config_text(&Settings::default())).unwrap();
        assert_eq!(
            (s.font.family.as_str(), s.font.size),
            ("Cascadia Mono", 14.0)
        );
        // Lo que falta toma el valor por defecto; un entero sirve como tamaño.
        let s: Settings = toml::from_str("[font]\nfamily = \"Monocraft\"").unwrap();
        assert_eq!((s.font.family.as_str(), s.font.size), ("Monocraft", 14.0));
        let s: Settings = toml::from_str("[font]\nsize = 16").unwrap();
        assert_eq!(s.font.size, 16.0);
        // Un config viejo, sin line_height, toma el valor por defecto.
        assert_eq!(s.font.line_height, 1.2);
    }

    #[test]
    fn saved_config_round_trips() {
        // Lo que guarda el menú se vuelve a leer igual, incluidas rutas con '\' y comillas.
        let mut s = Settings::default();
        for family in ["Monocraft", r#"C:\fuentes\Mi "Fuente".otf"#] {
            s.font.family = family.into();
            s.font.size = 18.0;
            s.font.line_height = 1.35;
            let back: Settings = toml::from_str(&config_text(&s)).unwrap();
            assert!(back == s, "no volvió igual: {family}");
        }
    }
}
