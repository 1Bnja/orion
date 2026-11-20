//! Ventana de configuración (Ctrl+, o Cmd+, en Mac, o clic derecho): el tema y elegir, agrandar y agregar fuentes.

use crate::app::Orion;
use crate::backdrop::{BACKDROPS, Backdrop, find_backdrop};
use crate::colors::{THEMES, find_theme, set_surface_alpha, set_theme, theme};
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

/// Cambia el tema: los colores de la terminal y, si el fondo es claro, las ventanas de egui también.
pub(crate) fn apply_theme(ctx: &egui::Context, name: &str) {
    set_theme(find_theme(name).unwrap_or(0));
    let visuals = if theme().is_light() {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    ctx.set_visuals(visuals);
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
        if new.theme != self.settings.theme {
            apply_theme(ctx, &new.theme);
        }
        let look_changed =
            new.theme != self.settings.theme || new.backdrop != self.settings.backdrop;
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
