//! Ventana de configuración (Ctrl+, o Cmd+, en Mac, o clic derecho): el tema y elegir, agrandar y agregar fuentes.

use crate::app::Orion;
use crate::backdrop::{BACKDROPS, Backdrop, find_backdrop};
use crate::colors::{THEMES, find_theme, parse_hex, set_surface_alpha, set_theme, theme, to_hex};
use crate::config::Settings;
use crate::config::save_settings;
use crate::fonts::FontChoice;
use crate::fonts::add_font;
use crate::fonts::apply_font;
use crate::fonts::font_choices;
use crate::pty::{SHELLS, installed_shells};
use eframe::egui;
use std::path::Path;

/// Estado del menú de configuración.
#[derive(Default)]
pub(crate) struct SettingsMenu {
    pub(crate) open: bool,
    /// Se llena la primera vez que se abre (leer las fuentes toma unos milisegundos).
    pub(crate) fonts: Vec<FontChoice>,
    pub(crate) search: String,
    pub(crate) show_all: bool,
}

/// Cambia el tema: los colores de la terminal y los de las ventanas de egui (configuración, paleta).
pub(crate) fn apply_theme(ctx: &egui::Context, settings: &Settings) {
    set_theme(
        find_theme(&settings.theme).unwrap_or(0),
        settings.colors.overrides(),
    );
    ctx.set_visuals(theme().visuals());
}

impl Orion {
    pub(crate) fn open_menu(&mut self) {
        if self.menu.fonts.is_empty() {
            self.menu.fonts = font_choices();
        }
        self.menu.open = true;
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
        let colors_changed = new.colors != self.settings.colors;
        if new.theme != self.settings.theme || colors_changed {
            apply_theme(ctx, &new);
        }
        let look_changed = new.theme != self.settings.theme
            || new.backdrop != self.settings.backdrop
            || colors_changed;
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

    /// Ventana de configuración (Ctrl+, o Cmd+, en Mac, o clic derecho → Configuración…).
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.menu.open;
        let mut new = self.settings.clone();
        let menu = &mut self.menu;
        egui::Window::new("Configuración")
            .open(&mut open)
            .collapsible(false)
            .default_width(340.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Tema");
                    egui::ComboBox::from_id_salt("tema")
                        .selected_text(&new.theme)
                        .show_ui(ui, |ui| {
                            for t in &THEMES {
                                ui.selectable_value(&mut new.theme, t.name.to_string(), t.name);
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("Fondo");
                    egui::ComboBox::from_id_salt("fondo")
                        .selected_text(&new.backdrop)
                        .show_ui(ui, |ui| {
                            for (name, _) in BACKDROPS {
                                ui.selectable_value(&mut new.backdrop, name.to_string(), name);
                            }
                        });
                })
                .response
                .on_hover_text(
                    "Mica: tinte suave y casi sólido. Acrílico: desenfoca lo de atrás. En Windows 11 y macOS.",
                );
                ui.horizontal(|ui| {
                    ui.label("Shell");
                    egui::ComboBox::from_id_salt("shell")
                        .selected_text(&new.shell)
                        .show_ui(ui, |ui| {
                            for &i in installed_shells() {
                                let name = SHELLS[i].name;
                                ui.selectable_value(&mut new.shell, name.to_string(), name);
                            }
                        });
                })
                .response
                .on_hover_text(keys!(
                    "La de las pestañas nuevas (",
                    "+Shift+T",
                    ")"
                ));
                ui.separator();
                ui.heading("Colores");
                ui.weak(
                    "Hex #RRGGBB. Lo que no cambies usa el tema y se mantiene al cambiar de tema.",
                );
                egui::ScrollArea::vertical()
                    .id_salt("colores")
                    .max_height(220.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (key, value) in new.colors.fields_mut() {
                            color_row(ui, key, value);
                        }
                        if ui.button("Restablecer todos").clicked() {
                            new.colors.clear();
                        }
                    });
                ui.separator();
                ui.heading("Fuente");
                ui.horizontal(|ui| {
                    ui.label("Tamaño");
                    ui.add(egui::Slider::new(&mut new.font.size, 8.0..=48.0).step_by(1.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Interlineado");
                    let line_height = egui::Slider::new(&mut new.font.line_height, 1.0..=2.0);
                    ui.add(line_height.step_by(0.05).fixed_decimals(2));
                });
                ui.add(
                    egui::TextEdit::singleline(&mut menu.search)
                        .hint_text("Buscar fuente…")
                        .desired_width(f32::INFINITY),
                );
                ui.checkbox(&mut menu.show_all, "Mostrar también las no monoespaciadas");
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let search = menu.search.to_lowercase();
                        for font in &menu.fonts {
                            let visible = font.monospace || menu.show_all;
                            if !visible || !font.label.to_lowercase().contains(&search) {
                                continue;
                            }
                            let selected = new.font.family == font.family;
                            if ui.selectable_label(selected, &font.label).clicked() {
                                new.font.family = font.family.clone();
                            }
                        }
                    });
                let current = menu.fonts.iter().find(|f| f.family == new.font.family);
                if current.is_some_and(|f| !f.monospace) {
                    // En una terminal todas las letras deben medir lo mismo, o la grilla se descuadra.
                    ui.colored_label(
                        theme().ansi[3],
                        "⚠ No es monoespaciada: las letras pueden encimarse.",
                    );
                }
                ui.separator();
                ui.label("Para agregar otra fuente, arrastra un archivo .ttf u .otf a la ventana.");
                ui.weak("Los cambios se guardan en config.toml.");
            });
        self.menu.open = open;
        if new != self.settings {
            self.change_settings(ctx, new);
        }
    }
}

/// Una fila de la sección Colores: nombre, muestra editable, hex y vuelta al tema.
fn color_row(ui: &mut egui::Ui, key: &str, value: &mut Option<String>) {
    let base = color_base(key);
    ui.horizontal(|ui| {
        ui.label(color_label(key));
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
            .desired_width(78.0)
            .font(egui::TextStyle::Monospace)
            .text_color(if valid {
                ui.visuals().text_color()
            } else {
                ui.visuals().error_fg_color
            });
        if ui.add(edit).changed() {
            // Se guarda tal cual para poder escribir sin trabas; un hex inválido no se aplica.
            *value = Some(text);
        }
        if ui
            .add_enabled(value.is_some(), egui::Button::new("Tema"))
            .on_hover_text("Volver al color del tema")
            .clicked()
        {
            *value = None;
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
