//! Configuración del usuario: leer y guardar %APPDATA%\orion\config.toml.

use crate::backdrop::{BACKDROPS, find_backdrop};
use crate::colors::{THEMES, find_theme};
use crate::pty::{SHELLS, find_shell};
use std::path::PathBuf;
use std::time::SystemTime;

/// Texto de config.toml. Se regenera completo al guardar desde el menú.
/// ponytail: los comentarios que agregues a mano se pierden al guardar; toml_edit los conservaría.
pub(crate) fn config_text(settings: &Settings) -> String {
    // toml::Value escribe el texto con las comillas y escapes correctos (ej. rutas con '\').
    let family = toml::Value::String(settings.font.family.clone());
    let theme = toml::Value::String(settings.theme.clone());
    let themes: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
    let themes = themes.join(", ");
    let shell = toml::Value::String(settings.shell.clone());
    let shells: Vec<&str> = SHELLS.iter().map(|s| s.name).collect();
    let shells = shells.join(", ");
    let backdrop = toml::Value::String(settings.backdrop.clone());
    let backdrops: Vec<&str> = BACKDROPS.iter().map(|b| b.0).collect();
    let backdrops = backdrops.join(", ");
    let check_updates = settings.check_updates;
    let mut agents = String::new();
    for agent in &settings.agents {
        let name = toml::Value::String(agent.name.clone());
        let command = toml::Value::String(agent.command.clone());
        let worktree = agent.worktree;
        agents +=
            &format!("\n[[agent]]\nname = {name}\ncommand = {command}\nworktree = {worktree}\n");
    }
    if agents.is_empty() {
        agents =
            "\n# [[agent]]\n# name = \"Mi agente\"\n# command = \"mi-agente\"\n# worktree = true\n"
                .into();
    }
    let text = format!(
        r#"# Configuración de Orion. Se puede editar aquí o desde el menú (Ctrl+, o clic derecho).
# Si editas a mano, los cambios se aplican apenas guardas el archivo.

# Colores. Hay: {themes}.
theme = {theme}
# Fondo de Windows 11 detrás de la terminal. Hay: {backdrops}.
# Mica toma un tinte suave de tu fondo de pantalla; acrílico desenfoca lo que hay detrás de la ventana.
backdrop = {backdrop}
# Al abrir, buscar si hay una versión nueva de Orion (se instala solo si tú lo pides).
check_updates = {check_updates}
# Shell de las pestañas nuevas (Ctrl+Shift+T). Hay: {shells}.
# Para abrir otra solo una vez: clic derecho en "+ Nueva pestaña", o la paleta (Ctrl+Shift+P).
shell = {shell}

[font]
# Nombre de una fuente instalada (como aparece en Windows), o la ruta a un archivo .ttf/.otf.
# Ejemplos: "Cascadia Mono", "Monocraft", 'C:\fuentes\MiFuente.ttf'
family = {family}
# Las fuentes pixeladas se ven nítidas en múltiplos de su grilla (Monocraft: 18, 27…).
size = {:.1}
# Alto de cada línea respecto de la letra: 1.0 = pegadas, 1.2 = con aire, 1.5 = muy espaciadas.
line_height = {:.2}

# Agentes: cada [[agent]] aparece en la paleta (Ctrl+Shift+P) y en el clic derecho de
# "+ Nueva pestaña". Abre una pestaña en la carpeta actual y escribe `command`.
# Con worktree = true, antes crea un git worktree al lado del repo, en una rama nueva
# (orion/nombre-1, -2…): así varios agentes trabajan en paralelo sin pisarse.
"#,
        settings.font.size, settings.font.line_height
    );
    text + &agents
}

/// Guarda la configuración en config.toml (creando la carpeta si hace falta).
pub(crate) fn save_settings(settings: &Settings) -> std::io::Result<()> {
    let path = config_path().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::create_dir_all(path.parent().expect("la ruta tiene carpeta"))?;
    std::fs::write(path, config_text(settings))
}

#[derive(serde::Deserialize, Clone, PartialEq)]
#[serde(default)] // lo que falte en el archivo toma el valor por defecto
pub(crate) struct Settings {
    /// Nombre de uno de los temas de `colors::THEMES`.
    pub(crate) theme: String,
    /// Nombre de una de las shells de `pty::SHELLS`.
    pub(crate) shell: String,
    /// Nombre de uno de los fondos de `backdrop::BACKDROPS`.
    pub(crate) backdrop: String,
    /// Al abrir, revisar si hay una versión nueva en GitHub.
    pub(crate) check_updates: bool,
    pub(crate) font: FontSettings,
    /// Perfiles de agente ([[agent]] en el archivo).
    #[serde(rename = "agent")]
    pub(crate) agents: Vec<Agent>,
}

#[derive(serde::Deserialize, Clone, PartialEq, Debug)]
pub(crate) struct Agent {
    pub(crate) name: String,
    /// Lo que se escribe en la shell para lanzarlo.
    pub(crate) command: String,
    /// Antes de lanzarlo, crear un git worktree en una rama nueva.
    #[serde(default)]
    pub(crate) worktree: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: THEMES[0].name.into(),
            shell: SHELLS[0].name.into(),
            backdrop: BACKDROPS[0].0.into(),
            check_updates: true,
            font: FontSettings::default(),
            agents: Vec::new(),
        }
    }
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
            family: crate::fonts::default_family().into(),
            size: 14.0,
            line_height: 1.2,
        }
    }
}

/// Windows: %APPDATA%\orion\config.toml. macOS: ~/Library/Application Support/orion/config.toml.
pub(crate) fn config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let appdata = PathBuf::from(std::env::var_os("APPDATA")?);
    #[cfg(not(windows))]
    let appdata = PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support");
    Some(appdata.join("orion").join("config.toml"))
}

/// Cuándo se modificó config.toml por última vez (para notar que lo guardaste).
pub(crate) fn config_modified() -> Option<SystemTime> {
    std::fs::metadata(config_path()?).ok()?.modified().ok()
}

/// Interpreta el texto de config.toml. El error dice la línea y qué está mal, en una sola línea.
/// Los valores absurdos (tamaño 0, interlineado 50) se llevan al rango que funciona.
pub(crate) fn parse_settings(text: &str) -> Result<Settings, String> {
    let mut settings: Settings = toml::from_str(text).map_err(|e| {
        let line = e
            .span()
            .map_or(0, |s| text[..s.start].matches('\n').count() + 1);
        format!("línea {line}: {}", e.message().trim())
    })?;
    settings.font.size = settings.font.size.clamp(6.0, 72.0);
    settings.font.line_height = settings.font.line_height.clamp(1.0, 3.0);
    // El nombre queda escrito como en la lista ("dracula" → "Dracula").
    let theme = find_theme(&settings.theme)
        .ok_or_else(|| format!("no hay un tema \"{}\"", settings.theme))?;
    settings.theme = THEMES[theme].name.into();
    let shell = find_shell(&settings.shell)
        .ok_or_else(|| format!("no conozco la shell \"{}\"", settings.shell))?;
    settings.shell = SHELLS[shell].name.into();
    let backdrop = find_backdrop(&settings.backdrop)
        .ok_or_else(|| format!("no hay un fondo \"{}\"", settings.backdrop))?;
    settings.backdrop = BACKDROPS.iter().find(|b| b.1 == backdrop).unwrap().0.into();
    Ok(settings)
}

/// Vuelve a leer config.toml (lo editaste a mano).
pub(crate) fn reload_settings() -> Result<Settings, String> {
    let path = config_path().ok_or("no encuentro la carpeta de configuración")?;
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_settings(&text)
}

/// Lee la configuración. Si no existe la crea; si tiene errores, avisa y usa los valores por defecto
/// (una coma mal puesta no debería impedir abrir la terminal).
pub(crate) fn load_settings() -> Settings {
    let Some(path) = config_path() else {
        return Settings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_settings(&text).unwrap_or_else(|e| {
            eprintln!("orion: error en {}, {e}", path.display());
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
            (crate::fonts::default_family(), 14.0)
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
    fn bad_config_gives_a_short_error_and_odd_values_are_clamped() {
        let err = parse_settings("[font]\nfamily = \"Monocraft\"\nsize = grande\n").err();
        assert!(
            err.as_deref().is_some_and(|e| e.starts_with("línea 3:")),
            "{err:?}"
        );
        let s = parse_settings("[font]\nsize = 0\nline_height = 50").unwrap();
        assert_eq!((s.font.size, s.font.line_height), (6.0, 3.0));
        // Tema: sin importar mayúsculas; uno que no existe es un error.
        assert_eq!(
            parse_settings("theme = 'dracula'").unwrap().theme,
            "Dracula"
        );
        let err = parse_settings("theme = 'Nope'").err().unwrap();
        assert!(err.contains("Nope"), "{err}");
        // La shell, escrita como en la lista (las de Windows y Mac son distintas: se usa la última).
        let last = SHELLS[SHELLS.len() - 1].name;
        let text = format!("shell = '{}'", last.to_lowercase());
        assert_eq!(parse_settings(&text).unwrap().shell, last);
        assert!(parse_settings("shell = 'nada'").is_err());
        assert_eq!(
            parse_settings("backdrop = 'acrilico'").unwrap().backdrop,
            "Acrílico"
        );
        assert!(parse_settings("backdrop = 'vidrio'").is_err());
    }

    #[test]
    fn saved_config_round_trips() {
        // Lo que guarda el menú se vuelve a leer igual, incluidas rutas con '\' y comillas.
        let mut s = Settings::default();
        let back: Settings = toml::from_str(&config_text(&s)).unwrap();
        assert!(back.agents.is_empty()); // el ejemplo va comentado
        s.agents = vec![Agent {
            name: "Revisor".into(),
            command: r#"mi-agente --modo "revisar""#.into(),
            worktree: true,
        }];
        for family in ["Monocraft", r#"C:\fuentes\Mi "Fuente".otf"#] {
            s.font.family = family.into();
            s.font.size = 18.0;
            s.font.line_height = 1.35;
            let back: Settings = toml::from_str(&config_text(&s)).unwrap();
            assert!(back == s, "no volvió igual: {family}");
        }
    }
}
