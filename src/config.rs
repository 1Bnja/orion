//! Configuración del usuario: leer y guardar %APPDATA%\orion\config.toml.

use crate::backdrop::{BACKDROPS, find_backdrop};
use crate::colors::{Overrides, THEMES, Theme, find_theme, parse_hex, to_hex};
use crate::pty::{SHELLS, find_shell};
use eframe::egui::Color32;
use std::path::PathBuf;
use std::time::SystemTime;

/// La tabla [colors] de config.toml: los overrides válidos, o un ejemplo comentado si no hay.
fn colors_text(settings: &Settings) -> String {
    let mut lines = String::new();
    for (key, value) in settings.colors.fields() {
        if let Some(v) = value
            && parse_hex(v).is_some()
        {
            let v = toml::Value::String(v.clone());
            lines += &format!("{key} = {v}\n");
        }
    }
    if lines.is_empty() {
        return r##"
# Colores de la interfaz, en hex #RRGGBB. Lo que no cambies usa el tema y se
# mantiene al cambiar de tema.
# [colors]
# fg = "#EDEDED"
# bg = "#000000"
# ansi4 = "#52A8FF"
"##
        .into();
    }
    format!(
        r#"
# Colores de la interfaz, en hex #RRGGBB. Lo que no esté usa el tema y se mantiene
# al cambiar de tema.
[colors]
{lines}"#
    )
}

/// Texto de config.toml. Se regenera completo al guardar desde el menú.
/// ponytail: los comentarios que agregues a mano se pierden al guardar; toml_edit los conservaría.
pub(crate) fn config_text(settings: &Settings) -> String {
    // toml::Value escribe el texto con las comillas y escapes correctos (ej. rutas con '\').
    let family = toml::Value::String(settings.font.family.clone());
    let theme = toml::Value::String(settings.theme.clone());
    let themes: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
    let themes = themes.join(", ");
    let colors = colors_text(settings);
    let shell = toml::Value::String(settings.shell.clone());
    let shells: Vec<&str> = SHELLS.iter().map(|s| s.name).collect();
    let shells = shells.join(", ");
    let backdrop = toml::Value::String(settings.backdrop.clone());
    let backdrops: Vec<&str> = BACKDROPS.iter().map(|b| b.0).collect();
    let backdrops = backdrops.join(", ");
    let check_updates = settings.check_updates;
    let auto_executors = settings.auto_executors;
    let disabled_executors = toml::Value::Array(
        settings
            .disabled_executors
            .iter()
            .cloned()
            .map(toml::Value::String)
            .collect(),
    );
    let mut themes_extra = String::new();
    for t in &settings.custom_themes {
        themes_extra += &t.to_toml();
    }
    if themes_extra.is_empty() {
        themes_extra = "\n# Temas propios (Apariencia → Guardar como tema).\n# [[custom_theme]]\n# name = \"Mio\"\n# fg = \"#DCE3EE\"\n# bg = \"#121821\"\n# selection = \"#263A5C\"\n".into();
    }
    let mut agents = String::new();
    for agent in &settings.agents {
        let name = toml::Value::String(agent.name.clone());
        let command = toml::Value::String(agent.command.clone());
        let worktree = agent.worktree;
        agents +=
            &format!("\n[[agent]]\nname = {name}\ncommand = {command}\nworktree = {worktree}\n");
        for (key, value) in [
            ("id", &agent.id),
            ("use_for", &agent.use_for),
            ("executor", &agent.executor),
        ] {
            if let Some(value) = value {
                agents += &format!("{key} = {}\n", toml::Value::String(value.clone()));
            }
        }
    }
    if agents.is_empty() {
        agents =
            "\n# [[agent]]\n# name = \"Mi agente\"\n# command = \"mi-agente\"\n# worktree = true\n# id = \"mi-agente\"\n# use_for = \"programar y escribir tests\"\n# executor = \"mi-agente --prompt {prompt}\"\n"
                .into();
    }
    let mut executors = String::new();
    for e in &settings.executors {
        let id = toml::Value::String(e.id.clone());
        let use_for = toml::Value::String(e.use_for.clone());
        let command = toml::Value::String(e.command.clone());
        executors +=
            &format!("\n[[executor]]\nid = {id}\nuse_for = {use_for}\ncommand = {command}\n");
    }
    // El modificador de los atajos de Orion: Cmd en Mac, Ctrl en Windows.
    let key = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    };
    let text = format!(
        r#"# Configuración de Orion. Se puede editar aquí o desde el menú ({key}+, o clic derecho).
# Si editas a mano, los cambios se aplican apenas guardas el archivo.

# Colores. Hay: {themes}.
theme = {theme}
# Material detrás de la terminal (Windows 11 y macOS). Hay: {backdrops}.
# Mica es un tinte suave y casi sólido; acrílico desenfoca lo que hay detrás de la ventana.
backdrop = {backdrop}
# Al abrir, buscar si hay una versión nueva de Orion (se instala solo si tú lo pides).
check_updates = {check_updates}
# Ofrecer automáticamente los CLIs conocidos como ejecutores cuando estén instalados.
# false conserva los modelos de Claude y los perfiles/ejecutores definidos aquí.
auto_executors = {auto_executors}
# Ids de los CLIs detectados que no quieres en el orquestador (Configuración → Agentes).
# No afecta a tus [[agent]] ni [[executor]].
disabled_executors = {disabled_executors}
# Shell de las pestañas nuevas ({key}+Shift+T). Hay: {shells}.
# Para abrir una pestaña con otro proyecto o shell: clic en "+ Nueva pestaña",
# o la paleta ({key}+Shift+P).
shell = {shell}
{colors}
[font]
# Nombre de una fuente instalada (como aparece en Windows), o la ruta a un archivo .ttf/.otf.
# Ejemplos: "Cascadia Mono", "Monocraft", 'C:\fuentes\MiFuente.ttf'
family = {family}
# Las fuentes pixeladas se ven nítidas en múltiplos de su grilla (Monocraft: 18, 27…).
size = {:.1}
# Alto de cada línea respecto de la letra: 1.0 = pegadas, 1.2 = con aire, 1.5 = muy espaciadas.
line_height = {:.2}

# Agentes: cada [[agent]] aparece en la paleta ({key}+Shift+P) y en el menú de
# "+ Nueva pestaña". Abre una pestaña en la carpeta actual y escribe `command`.
# Con worktree = true, antes crea un git worktree en .orion/worktrees/ dentro del repo,
# en una rama nueva (orion/nombre-1, -2…): así varios agentes trabajan en paralelo sin pisarse.
# Para que lleve tu .env y similares, lístalos en un archivo .worktreeinclude en la raíz del repo
# (una ruta relativa por línea, archivos o carpetas).
# Para usar el mismo perfil en el orquestador, agrega use_for y executor (plantilla interactiva
# con {{prompt}}). id es opcional: por defecto se deriva del nombre ("Mi agente" → "mi-agente").
# Un perfil sin ambos campos sigue siendo solo un acceso en la paleta.

# Ejecutores: el orquestador (palette → "Orquestar tarea con agentes") reparte las subtareas entre ellos.
# Claude Opus planifica y elige, para cada subtarea, el ejecutor más barato que sirva según `use_for`.
# Los tres modelos de Claude vienen siempre incluidos. Con auto_executors = true también se
# incluyen los providers con prompt interactivo verificado (Codex, OpenCode, Gemini CLI, etc.).
# Precedencia por id: Claude < providers < [[agent]] < [[executor]]; el último reemplaza al anterior.
# Solo se ofrecen los ejecutores cuyo programa (la primera palabra de `command`) está instalado.
# {{prompt}} se reemplaza por el prompt de la subtarea, en una línea y entre comillas;
# si falta, el prompt se agrega al final. Los ! y % se quitan del prompt.
# El comando debe abrir el agente en modo INTERACTIVO con el prompt inicial, y el agente debe
# terminar haciendo commit en su rama; si no, las subtareas que dependen de él se bloquean.
# Ejemplo (OpenCode):
# [[executor]]
# id = "deepseek-pro"
# use_for = "programar algo normal, barato"
# command = "opencode -m deepseek/deepseek-v4-pro --prompt {{prompt}}"
"#,
        settings.font.size, settings.font.line_height
    );
    text + &themes_extra + &agents + &executors
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
    /// Incluir los providers con plantilla interactiva en el catálogo del orquestador.
    pub(crate) auto_executors: bool,
    /// Ids de providers detectados que el usuario sacó del orquestador.
    pub(crate) disabled_executors: Vec<String>,
    /// Cambios de color de la interfaz ([colors]); lo que falte usa el tema.
    pub(crate) colors: ColorSettings,
    pub(crate) font: FontSettings,
    /// Temas propios ([[custom_theme]] en el archivo).
    #[serde(rename = "custom_theme", default)]
    pub(crate) custom_themes: Vec<SavedTheme>,
    /// Perfiles de agente ([[agent]] en el archivo).
    #[serde(rename = "agent")]
    pub(crate) agents: Vec<Agent>,
    /// Ejecutores extra del orquestador ([[executor]] en el archivo).
    #[serde(rename = "executor")]
    pub(crate) executors: Vec<Executor>,
}

/// Un tema guardado por el usuario: colores completos, no solo overrides.
#[derive(serde::Deserialize, Clone, PartialEq, Debug)]
pub(crate) struct SavedTheme {
    pub(crate) name: String,
    pub(crate) fg: String,
    pub(crate) bg: String,
    pub(crate) selection: String,
    #[serde(default)]
    pub(crate) mono: bool,
    pub(crate) cursor: Option<String>,
    pub(crate) sidebar: Option<String>,
    pub(crate) accent: Option<String>,
    pub(crate) line: Option<String>,
    pub(crate) hover: Option<String>,
    pub(crate) ansi0: String,
    pub(crate) ansi1: String,
    pub(crate) ansi2: String,
    pub(crate) ansi3: String,
    pub(crate) ansi4: String,
    pub(crate) ansi5: String,
    pub(crate) ansi6: String,
    pub(crate) ansi7: String,
    pub(crate) ansi8: String,
    pub(crate) ansi9: String,
    pub(crate) ansi10: String,
    pub(crate) ansi11: String,
    pub(crate) ansi12: String,
    pub(crate) ansi13: String,
    pub(crate) ansi14: String,
    pub(crate) ansi15: String,
}

impl SavedTheme {
    /// Snapshot of the resolved theme under a new name.
    pub(crate) fn from_resolved(name: String, theme: &Theme) -> Self {
        let ansi = |i: usize| to_hex(theme.ansi[i]);
        Self {
            name,
            fg: to_hex(theme.fg),
            bg: to_hex(theme.bg),
            selection: to_hex(theme.selection),
            mono: theme.mono,
            cursor: Some(to_hex(theme.cursor())),
            sidebar: Some(to_hex(theme.sidebar())),
            accent: Some(to_hex(theme.accent())),
            line: Some(to_hex(theme.line())),
            hover: Some(to_hex(theme.hover())),
            ansi0: ansi(0),
            ansi1: ansi(1),
            ansi2: ansi(2),
            ansi3: ansi(3),
            ansi4: ansi(4),
            ansi5: ansi(5),
            ansi6: ansi(6),
            ansi7: ansi(7),
            ansi8: ansi(8),
            ansi9: ansi(9),
            ansi10: ansi(10),
            ansi11: ansi(11),
            ansi12: ansi(12),
            ansi13: ansi(13),
            ansi14: ansi(14),
            ansi15: ansi(15),
        }
    }

    fn ansi_hex(&self) -> [&str; 16] {
        [
            &self.ansi0,
            &self.ansi1,
            &self.ansi2,
            &self.ansi3,
            &self.ansi4,
            &self.ansi5,
            &self.ansi6,
            &self.ansi7,
            &self.ansi8,
            &self.ansi9,
            &self.ansi10,
            &self.ansi11,
            &self.ansi12,
            &self.ansi13,
            &self.ansi14,
            &self.ansi15,
        ]
    }

    /// Parses into a runtime Theme. `name` should already be `'static` (leak once when applying).
    pub(crate) fn as_theme(&self, name: &'static str) -> Option<Theme> {
        let mut ansi = [Color32::BLACK; 16];
        for (slot, hex) in ansi.iter_mut().zip(self.ansi_hex()) {
            *slot = parse_hex(hex)?;
        }
        Some(Theme {
            name,
            fg: parse_hex(&self.fg)?,
            bg: parse_hex(&self.bg)?,
            selection: parse_hex(&self.selection)?,
            ansi,
            mono: self.mono,
            cursor: self.cursor.as_deref().and_then(parse_hex),
            derived: crate::colors::Derived {
                sidebar: self.sidebar.as_deref().and_then(parse_hex),
                accent: self.accent.as_deref().and_then(parse_hex),
                line: self.line.as_deref().and_then(parse_hex),
                hover: self.hover.as_deref().and_then(parse_hex),
            },
        })
    }

    fn to_toml(&self) -> String {
        let name = toml::Value::String(self.name.clone());
        let opt = |key: &str, value: &Option<String>| {
            value
                .as_ref()
                .filter(|v| parse_hex(v).is_some())
                .map(|v| format!("{key} = {}\n", toml::Value::String(v.clone())))
                .unwrap_or_default()
        };
        let mut body = format!(
            "\n[[custom_theme]]\nname = {name}\nfg = {}\nbg = {}\nselection = {}\nmono = {}\n",
            toml::Value::String(self.fg.clone()),
            toml::Value::String(self.bg.clone()),
            toml::Value::String(self.selection.clone()),
            self.mono,
        );
        body += &opt("cursor", &self.cursor);
        body += &opt("sidebar", &self.sidebar);
        body += &opt("accent", &self.accent);
        body += &opt("line", &self.line);
        body += &opt("hover", &self.hover);
        for (i, hex) in self.ansi_hex().into_iter().enumerate() {
            body += &format!("ansi{i} = {}\n", toml::Value::String(hex.to_string()));
        }
        body
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("theme.name vacío".into());
        }
        for (key, value) in [
            ("fg", self.fg.as_str()),
            ("bg", self.bg.as_str()),
            ("selection", self.selection.as_str()),
        ] {
            if parse_hex(value).is_none() {
                return Err(format!("theme.{}: '{value}' no es #RRGGBB", key));
            }
        }
        for (i, hex) in self.ansi_hex().into_iter().enumerate() {
            if parse_hex(hex).is_none() {
                return Err(format!("theme.ansi{i}: '{hex}' no es #RRGGBB"));
            }
        }
        Ok(())
    }
}

/// Algo que ejecuta subtareas del orquestador, además de los modelos de Claude.
#[derive(serde::Deserialize, Clone, PartialEq, Debug)]
pub(crate) struct Executor {
    /// Cómo lo nombra el plan y lo muestra la ventana.
    pub(crate) id: String,
    /// Para qué tareas conviene (le llega al planificador).
    pub(crate) use_for: String,
    /// Lo que se escribe en la shell; `{prompt}` es donde va el prompt.
    pub(crate) command: String,
}

#[derive(serde::Deserialize, Clone, PartialEq, Debug)]
pub(crate) struct Agent {
    pub(crate) name: String,
    /// Id del ejecutor; si falta se deriva del nombre.
    pub(crate) id: Option<String>,
    /// Con `executor`, habilita este perfil para el orquestador.
    pub(crate) use_for: Option<String>,
    /// Plantilla interactiva con `{prompt}`, independiente del comando de la paleta.
    pub(crate) executor: Option<String>,
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
            auto_executors: true,
            disabled_executors: Vec::new(),
            colors: ColorSettings::default(),
            font: FontSettings::default(),
            custom_themes: Vec::new(),
            agents: Vec::new(),
            executors: Vec::new(),
        }
    }
}

/// Overrides de color de la interfaz, en hex "#RRGGBB". None = el color del tema.
#[derive(serde::Deserialize, Clone, PartialEq, Default, Debug)]
#[serde(default)]
pub(crate) struct ColorSettings {
    pub(crate) fg: Option<String>,
    pub(crate) bg: Option<String>,
    pub(crate) selection: Option<String>,
    pub(crate) sidebar: Option<String>,
    pub(crate) accent: Option<String>,
    pub(crate) line: Option<String>,
    pub(crate) hover: Option<String>,
    pub(crate) ansi0: Option<String>,
    pub(crate) ansi1: Option<String>,
    pub(crate) ansi2: Option<String>,
    pub(crate) ansi3: Option<String>,
    pub(crate) ansi4: Option<String>,
    pub(crate) ansi5: Option<String>,
    pub(crate) ansi6: Option<String>,
    pub(crate) ansi7: Option<String>,
    pub(crate) ansi8: Option<String>,
    pub(crate) ansi9: Option<String>,
    pub(crate) ansi10: Option<String>,
    pub(crate) ansi11: Option<String>,
    pub(crate) ansi12: Option<String>,
    pub(crate) ansi13: Option<String>,
    pub(crate) ansi14: Option<String>,
    pub(crate) ansi15: Option<String>,
}

impl ColorSettings {
    /// Las claves de [colors] con su valor, en el orden en que se muestran y guardan.
    pub(crate) fn fields(&self) -> [(&'static str, &Option<String>); 23] {
        [
            ("fg", &self.fg),
            ("bg", &self.bg),
            ("selection", &self.selection),
            ("sidebar", &self.sidebar),
            ("accent", &self.accent),
            ("line", &self.line),
            ("hover", &self.hover),
            ("ansi0", &self.ansi0),
            ("ansi1", &self.ansi1),
            ("ansi2", &self.ansi2),
            ("ansi3", &self.ansi3),
            ("ansi4", &self.ansi4),
            ("ansi5", &self.ansi5),
            ("ansi6", &self.ansi6),
            ("ansi7", &self.ansi7),
            ("ansi8", &self.ansi8),
            ("ansi9", &self.ansi9),
            ("ansi10", &self.ansi10),
            ("ansi11", &self.ansi11),
            ("ansi12", &self.ansi12),
            ("ansi13", &self.ansi13),
            ("ansi14", &self.ansi14),
            ("ansi15", &self.ansi15),
        ]
    }

    /// Lo mismo que `fields`, pero para editarlos en el menú.
    pub(crate) fn fields_mut(&mut self) -> [(&'static str, &mut Option<String>); 23] {
        [
            ("fg", &mut self.fg),
            ("bg", &mut self.bg),
            ("selection", &mut self.selection),
            ("sidebar", &mut self.sidebar),
            ("accent", &mut self.accent),
            ("line", &mut self.line),
            ("hover", &mut self.hover),
            ("ansi0", &mut self.ansi0),
            ("ansi1", &mut self.ansi1),
            ("ansi2", &mut self.ansi2),
            ("ansi3", &mut self.ansi3),
            ("ansi4", &mut self.ansi4),
            ("ansi5", &mut self.ansi5),
            ("ansi6", &mut self.ansi6),
            ("ansi7", &mut self.ansi7),
            ("ansi8", &mut self.ansi8),
            ("ansi9", &mut self.ansi9),
            ("ansi10", &mut self.ansi10),
            ("ansi11", &mut self.ansi11),
            ("ansi12", &mut self.ansi12),
            ("ansi13", &mut self.ansi13),
            ("ansi14", &mut self.ansi14),
            ("ansi15", &mut self.ansi15),
        ]
    }

    /// Vuelve todos los colores al tema.
    pub(crate) fn clear(&mut self) {
        for (_, value) in self.fields_mut() {
            *value = None;
        }
    }

    /// Los overrides ya convertidos a Color32. Un hex inválido se ignora (usa el tema).
    pub(crate) fn overrides(&self) -> Overrides {
        let mut overrides = Overrides::default();
        for (key, value) in self.fields() {
            let color = value.as_deref().and_then(parse_hex);
            match key {
                "fg" => overrides.fg = color,
                "bg" => overrides.bg = color,
                "selection" => overrides.selection = color,
                "sidebar" => overrides.sidebar = color,
                "accent" => overrides.accent = color,
                "line" => overrides.line = color,
                "hover" => overrides.hover = color,
                _ => {
                    if let Some(i) = key
                        .strip_prefix("ansi")
                        .and_then(|n| n.parse::<usize>().ok())
                    {
                        overrides.ansi[i] = color;
                    }
                }
            }
        }
        overrides
    }

    /// Revisa que cada valor sea "#RRGGBB"; el error nombra la clave.
    pub(crate) fn validate(&self) -> Result<(), String> {
        for (key, value) in self.fields() {
            if let Some(v) = value
                && parse_hex(v).is_none()
            {
                return Err(format!("colors.{key}: '{v}' no es un color #RRGGBB"));
            }
        }
        Ok(())
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
    settings.colors.validate()?;
    for custom in &settings.custom_themes {
        custom.validate()?;
    }
    // El nombre queda escrito como en la lista ("dracula" → "Dracula").
    if let Some(i) = find_theme(&settings.theme) {
        settings.theme = THEMES[i].name.into();
    } else if let Some(custom) = settings
        .custom_themes
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(settings.theme.trim()))
    {
        settings.theme = custom.name.clone();
    } else {
        return Err(format!("no hay un tema \"{}\"", settings.theme));
    }
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
        assert!(s.auto_executors); // los configs anteriores activan la detección automática
        assert!(s.disabled_executors.is_empty());
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
    fn color_overrides_parse_and_bad_hex_says_which_key() {
        let s = parse_settings("[colors]\nfg = '#010203'\nansi4 = '#A0B0C0'").unwrap();
        let o = s.colors.overrides();
        assert_eq!(o.fg, parse_hex("#010203"));
        assert_eq!(o.ansi[4], parse_hex("#A0B0C0"));
        assert_eq!(o.bg, None);
        let err = parse_settings("[colors]\nbg = '#12'").err().unwrap();
        assert_eq!(err, "colors.bg: '#12' no es un color #RRGGBB");
        let err = parse_settings("[colors]\nansi7 = 'nope'").err().unwrap();
        assert!(err.starts_with("colors.ansi7:"), "{err}");
    }

    #[test]
    fn color_overrides_save_and_load() {
        let mut s = Settings::default();
        s.colors.bg = Some("#123456".into());
        let text = config_text(&s);
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back.colors, s.colors);
        // Sin overrides, la tabla va comentada (el ejemplo no se lee como configuración).
        let text = config_text(&Settings::default());
        assert!(text.contains("# [colors]"));
        assert!(!text.lines().any(|l| l.trim() == "[colors]"));
    }

    #[test]
    fn saved_config_round_trips() {
        // Lo que guarda el menú se vuelve a leer igual, incluidas rutas con '\' y comillas.
        let mut s = Settings::default();
        let back: Settings = toml::from_str(&config_text(&s)).unwrap();
        assert!(back.agents.is_empty() && back.executors.is_empty()); // el ejemplo va comentado
        s.agents = vec![Agent {
            name: "Revisor".into(),
            id: Some("revisor-propio".into()),
            use_for: Some(r#"revisar "tests" en C:\repo"#.into()),
            executor: Some(r#"mi-agente --modo "revisar" --prompt {prompt}"#.into()),
            command: r#"mi-agente --modo "revisar""#.into(),
            worktree: true,
        }];
        s.disabled_executors = vec!["codex".into(), r#"raro "id" \x"#.into()];
        s.executors = vec![Executor {
            id: "junie".into(),
            use_for: r#"cambios "grandes" en C:\repo"#.into(),
            command: "junie --model=mix.normal --prompt={prompt}".into(),
        }];
        for family in ["Monocraft", r#"C:\fuentes\Mi "Fuente".otf"#] {
            s.font.family = family.into();
            s.font.size = 18.0;
            s.font.line_height = 1.35;
            for auto in [true, false] {
                s.auto_executors = auto;
                let back = parse_settings(&config_text(&s)).unwrap();
                assert!(back == s, "no volvió igual: {family}, auto={auto}");
            }
        }
    }

    #[test]
    fn old_profiles_keep_optional_executor_fields_absent_when_saved() {
        let settings =
            parse_settings("[[agent]]\nname = 'Solo paleta'\ncommand = 'mi-agente'").unwrap();
        let agent = &settings.agents[0];
        assert_eq!(agent.id, None);
        assert_eq!(agent.use_for, None);
        assert_eq!(agent.executor, None);
        assert!(!agent.worktree);
        let text = config_text(&settings);
        for key in ["id", "use_for", "executor"] {
            assert!(
                !text
                    .lines()
                    .any(|line| line.starts_with(&format!("{key} =")))
            );
        }
        assert!(parse_settings(&text).unwrap() == settings);
    }

    #[test]
    fn unified_profiles_round_trip_with_implicit_and_explicit_ids() {
        let settings = parse_settings(
            r#"
auto_executors = false
[[agent]]
name = "Mi agente"
command = "mi-agente"
use_for = "programar"
executor = "mi-agente --prompt {prompt}"
[[agent]]
name = "Otro agente"
command = "otro"
id = "mi-id"
executor = "otro {prompt}"
"#,
        )
        .unwrap();
        assert!(!settings.auto_executors);
        assert!(parse_settings(&config_text(&settings)).unwrap() == settings);
    }
}
