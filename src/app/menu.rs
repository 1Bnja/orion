//! Ventana de configuración (Ctrl+, o Cmd+, en Mac, o clic derecho): el tema y elegir, agrandar y agregar fuentes.

use crate::app::Orion;
use crate::backdrop::{Backdrop, find_backdrop};
use crate::colors::{
    THEMES, find_theme, parse_hex, set_active_theme, set_surface_alpha, theme, to_hex,
};
use crate::config::{SavedTheme, Settings, save_settings};
use crate::fonts::FontChoice;
use crate::fonts::add_font;
use crate::fonts::apply_font;
use crate::fonts::font_choices;
use eframe::egui;
use std::path::Path;

use super::settings::Section;

/// Estado del menú de configuración.
#[derive(Default)]
pub(crate) struct SettingsMenu {
    pub(crate) open: bool,
    /// Se llena la primera vez que se abre (leer las fuentes toma unos milisegundos).
    pub(crate) fonts: Vec<FontChoice>,
    pub(crate) search: String,
    pub(crate) show_all: bool,
    /// Texto del buscador de secciones.
    pub(crate) filter: String,
    /// La última sección elegida: se recuerda mientras Orion siga abierto.
    pub(crate) section: Section,
    /// Estado de la sección Agentes (formulario y modelos de Ollama).
    pub(super) agents: super::settings_agents::AgentsPanel,
    /// Name typed in "Guardar como tema".
    pub(crate) theme_name: String,
    /// Last error from saving a theme, if any.
    pub(crate) theme_error: Option<String>,
    /// Whether "Más colores" is expanded in the Colores section.
    pub(crate) more_colors: bool,
    /// El tema (oscuro o no) que ya se le pidió a la ventana de ajustes: pedirlo en cada frame
    /// hace que Windows la vuelva a pintar entera (SetWindowTheme) y la ventana tiembla.
    pub(crate) theme_sent: Option<bool>,
}

/// Resolves the theme the settings would show (built-in or custom + overrides), without applying it.
pub(crate) fn preview_theme(settings: &Settings) -> crate::colors::Theme {
    let overrides = settings.colors.overrides();
    if let Some(custom) = settings
        .custom_themes
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(settings.theme.trim()))
        && let Some(base) = custom.as_theme("")
    {
        return base.with_overrides(overrides);
    }
    let base = find_theme(&settings.theme).map_or(&THEMES[0], |i| &THEMES[i]);
    base.with_overrides(overrides)
}

/// Cambia el tema: los colores de la terminal y los de las ventanas de egui (configuración, paleta).
pub(crate) fn apply_theme(ctx: &egui::Context, settings: &Settings) {
    let overrides = settings.colors.overrides();
    let resolved = if let Some(custom) = settings
        .custom_themes
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(settings.theme.trim()))
    {
        let name: &'static str = Box::leak(custom.name.clone().into_boxed_str());
        custom
            .as_theme(name)
            .unwrap_or_else(|| THEMES[0].with_overrides(overrides))
            .with_overrides(overrides)
    } else {
        preview_theme(settings)
    };
    set_active_theme(resolved);
    // For both: egui follows the system's light or dark mode, and Orion's theme decides here.
    for mode in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.set_visuals_of(mode, theme().visuals());
    }
}

/// Color preview card: empty terminal strip + equal swatches (updates as you edit).
pub(crate) fn theme_preview(ui: &mut egui::Ui, settings: &Settings) {
    let t = preview_theme(settings);
    let frame = egui::Frame::new()
        .fill(t.bg)
        .stroke(egui::Stroke::new(1.0, t.line()))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(12, 10));
    frame.show(ui, |ui| {
        // Empty terminal: only the prompt and the cursor, nothing typed.
        let mono = egui::FontId::monospace(13.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(
                egui::RichText::new("~ ")
                    .font(mono.clone())
                    .color(t.ansi[4]),
            );
            ui.label(
                egui::RichText::new("❯ ")
                    .font(mono.clone())
                    .color(t.ansi[3]),
            );
            let (cursor, _) = ui.allocate_exact_size(egui::vec2(7.0, 13.0), egui::Sense::hover());
            ui.painter().rect_filled(cursor, 1.0, t.cursor());
        });
        ui.add_space(10.0);
        let swatch = |ui: &mut egui::Ui, color: egui::Color32, tip: &str| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
            let stroke = egui::Stroke::new(1.0, t.line());
            ui.painter()
                .rect(rect, 3.0, color, stroke, egui::StrokeKind::Inside);
            response.on_hover_text(tip);
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (color, tip) in [
                (t.bg, "Fondo"),
                (t.fg, "Texto"),
                (t.selection, "Selección"),
                (t.accent(), "Acento"),
            ] {
                swatch(ui, color, tip);
            }
            ui.add_space(2.0);
            for (i, &color) in t.ansi.iter().enumerate().take(8) {
                swatch(ui, color, &format!("ANSI {i}"));
            }
        });
    });
}

/// Saves the current look as a named theme and selects it.
pub(crate) fn save_as_theme(settings: &mut Settings, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Ponle un nombre al tema".into());
    }
    if THEMES.iter().any(|t| t.name.eq_ignore_ascii_case(name)) {
        return Err("Ese nombre ya es un tema incluido".into());
    }
    let resolved = preview_theme(settings);
    let saved = SavedTheme::from_resolved(name.into(), &resolved);
    saved.validate()?;
    settings
        .custom_themes
        .retain(|t| !t.name.eq_ignore_ascii_case(name));
    settings.custom_themes.push(saved);
    settings.theme = name.into();
    settings.colors.clear();
    Ok(())
}

impl Orion {
    pub(crate) fn open_menu(&mut self) {
        if self.menu.fonts.is_empty() {
            self.menu.fonts = font_choices();
        }
        self.menu.open = true;
        self.menu.theme_sent = None; // puede ser una ventana nueva: hay que pedirle el tema
    }

    /// Abre Configuración en la sección Agentes (y la trae al frente si ya estaba abierta).
    pub(crate) fn open_agents_settings(&mut self, ctx: &egui::Context) {
        self.open_menu();
        self.menu.section = Section::Agentes;
        self.menu.filter.clear(); // una búsqueda anterior podría ocultar la sección
        ctx.send_viewport_cmd_to(super::settings::viewport_id(), egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    /// Aplica una configuración nueva al instante (la fuente solo si cambió) y la guarda en config.toml.
    pub(crate) fn change_settings(&mut self, ctx: &egui::Context, new: Settings) {
        self.apply_settings(ctx, new);
        if let Err(e) = save_settings(&self.settings) {
            eprintln!("orion: no pude guardar la configuración: {e}");
        }
    }

    /// Aplica una configuración sin guardarla (la fuente y el tema solo si cambiaron).
    pub(crate) fn apply_settings(&mut self, ctx: &egui::Context, new: Settings) {
        if new.font.family != self.settings.font.family {
            apply_font(ctx, &new.font.family);
        }
        let theme_changed = new.theme != self.settings.theme
            || new.colors != self.settings.colors
            || new.custom_themes != self.settings.custom_themes;
        if theme_changed {
            apply_theme(ctx, &new);
        }
        let look_changed = theme_changed || new.backdrop != self.settings.backdrop;
        self.settings = new;
        if look_changed {
            self.apply_window_look();
        }
    }

    /// El material de la ventana (Mica o acrílico) y la barra de título clara u oscura según el tema.
    pub(crate) fn apply_window_look(&self) {
        let backdrop = find_backdrop(&self.settings.backdrop).unwrap_or(Backdrop::None);
        set_surface_alpha(backdrop.alpha());
        if let Some(window) = self.window {
            crate::backdrop::apply(window, backdrop, !theme().is_light());
        }
    }

    /// Una fuente arrastrada a la ventana: se copia a la carpeta de Orion y se usa.
    pub(crate) fn font_dropped(&mut self, ctx: &egui::Context, path: &Path) {
        match add_font(path) {
            Ok(dest) => {
                let mut new = self.settings.clone();
                new.font.family = dest.display().to_string();
                self.change_settings(ctx, new);
                self.menu.fonts = font_choices(); // para que aparezca en la lista
                self.open_menu();
            }
            Err(e) => eprintln!("orion: no pude agregar {}: {e}", path.display()),
        }
    }
}

/// One row inside an `egui::Grid`: label | swatch | hex | reset. Call `end_row` is done here.
pub(crate) fn color_row(ui: &mut egui::Ui, key: &str, value: &mut Option<String>) {
    let base = color_base(key);
    ui.add_sized(
        egui::vec2(120.0, 22.0),
        egui::Label::new(color_label(key)).selectable(false),
    );

    let mut color = value.as_deref().and_then(parse_hex).unwrap_or(base);
    let picker = egui::color_picker::color_edit_button_srgba(
        ui,
        &mut color,
        egui::color_picker::Alpha::Opaque,
    );
    if picker.changed() {
        *value = Some(to_hex(color));
    }

    let valid = match value.as_deref() {
        Some(text) => parse_hex(text).is_some(),
        None => true,
    };
    let mut text = value.clone().unwrap_or_else(|| to_hex(base));
    let edit = egui::TextEdit::singleline(&mut text)
        .desired_width(92.0)
        .font(egui::TextStyle::Monospace)
        .text_color(if valid {
            ui.visuals().text_color()
        } else {
            ui.visuals().error_fg_color
        });
    if ui.add_sized(egui::vec2(100.0, 22.0), edit).changed() {
        // Kept as typed so you can edit freely; invalid hex is not applied.
        *value = Some(text);
    }

    if ui
        .add_enabled_ui(value.is_some(), |ui| {
            ui.add_sized(egui::vec2(56.0, 22.0), egui::Button::new("Tema"))
        })
        .inner
        .on_hover_text("Volver al color del tema")
        .clicked()
    {
        *value = None;
    }
    ui.end_row();
}

/// Aligned table of color rows (same columns on every line).
pub(crate) fn color_table<'a>(
    ui: &mut egui::Ui,
    id: &'static str,
    rows: impl IntoIterator<Item = (&'static str, &'a mut Option<String>)>,
) {
    egui::Grid::new(id)
        .num_columns(4)
        .spacing([12.0, 8.0])
        .min_col_width(20.0)
        .show(ui, |ui| {
            for (key, value) in rows {
                color_row(ui, key, value);
            }
        });
}

/// Nombre visible de una clave de [colors].
fn color_label(key: &str) -> String {
    match key {
        "fg" => "Texto".into(),
        "bg" => "Fondo".into(),
        "selection" => "Selección".into(),
        "sidebar" => "Barra lateral".into(),
        "accent" => "Acento".into(),
        "line" => "Separadores".into(),
        "hover" => "Resaltado".into(),
        _ => format!("ANSI {}", key.trim_start_matches("ansi")),
    }
}

/// El color del tema para una clave (sin contar su propio override).
fn color_base(key: &str) -> egui::Color32 {
    let t = theme();
    match key {
        "fg" => t.fg,
        "bg" => t.bg,
        "selection" => t.selection,
        "sidebar" => t.sidebar(),
        "accent" => t.accent(),
        "line" => t.line(),
        "hover" => t.hover(),
        _ => {
            let i = key.trim_start_matches("ansi").parse::<usize>().unwrap_or(0);
            t.ansi[i]
        }
    }
}
