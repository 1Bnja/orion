//! Ventana de configuración (Ctrl+, o clic derecho): elegir, agrandar y agregar fuentes.

use crate::app::Orion;
use crate::colors::PALETTE;
use crate::config::Settings;
use crate::config::save_settings;
use crate::fonts::FontChoice;
use crate::fonts::add_font;
use crate::fonts::apply_font;
use crate::fonts::font_choices;
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

impl Orion {
    pub(crate) fn open_menu(&mut self) {
        if self.menu.fonts.is_empty() {
            self.menu.fonts = font_choices();
        }
        self.menu.open = true;
    }

    /// Aplica una configuración nueva al instante (la fuente solo si cambió) y la guarda en config.toml.
    pub(crate) fn change_settings(&mut self, ctx: &egui::Context, new: Settings) {
        if new.font.family != self.settings.font.family {
            apply_font(ctx, &new.font.family);
        }
        self.settings = new;
        if let Err(e) = save_settings(&self.settings) {
            eprintln!("orion: no pude guardar la configuración: {e}");
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

    /// Ventana de configuración (Ctrl+, o clic derecho → Configuración…).
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
                ui.heading("Fuente");
                ui.horizontal(|ui| {
                    ui.label("Tamaño");
                    ui.add(egui::Slider::new(&mut new.font.size, 8.0..=48.0).step_by(1.0));
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
                        PALETTE[3],
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
