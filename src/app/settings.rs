//! Ventana de configuración: una barra de secciones a la izquierda y los ajustes de la elegida a la derecha.

use super::Orion;
use super::menu::{SettingsMenu, color_row};
use crate::backdrop::BACKDROPS;
use crate::colors::{THEMES, theme};
use crate::config::{Settings, config_path};
use crate::links::{Link, open};
use crate::pty::{SHELLS, installed_shells};
use eframe::egui;

/// Una sección de la ventana de configuración.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Section {
    #[default]
    Apariencia,
    Colores,
    Fuente,
    Terminal,
    Avanzado,
}

impl Section {
    /// Todas, en el orden en que se muestran.
    pub(crate) const ALL: [Self; 5] = [
        Self::Apariencia,
        Self::Colores,
        Self::Fuente,
        Self::Terminal,
        Self::Avanzado,
    ];

    /// El título que se ve en la barra izquierda y arriba a la derecha.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Apariencia => "Apariencia",
            Self::Colores => "Colores",
            Self::Fuente => "Fuente",
            Self::Terminal => "Terminal",
            Self::Avanzado => "Avanzado",
        }
    }

    /// Palabras que también encuentran la sección en el buscador.
    pub(crate) fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Apariencia => &["tema", "fondo", "mica", "acrilico", "ventana"],
            Self::Colores => &["color", "colores", "hex", "paleta", "ansi"],
            Self::Fuente => &[
                "letra",
                "letras",
                "fuente",
                "fuentes",
                "tipografia",
                "monoespaciada",
                "tamaño",
                "interlineado",
                "ttf",
                "otf",
            ],
            Self::Terminal => &[
                "shell",
                "terminal",
                "actualizaciones",
                "powershell",
                "cmd",
                "zsh",
                "wsl",
            ],
            Self::Avanzado => &[
                "avanzado",
                "config",
                "config.toml",
                "agentes",
                "ejecutores",
                "orquestador",
                "archivo",
            ],
        }
    }
}

/// Las secciones cuyo título o palabras clave contienen `query` (sin mayúsculas ni acentos
/// básicos). Una consulta vacía las devuelve todas.
pub(crate) fn sections_matching(query: &str) -> Vec<Section> {
    let query = normalize(query.trim());
    if query.is_empty() {
        return Section::ALL.to_vec();
    }
    Section::ALL
        .into_iter()
        .filter(|section| {
            normalize(section.title()).contains(&query)
                || section
                    .keywords()
                    .iter()
                    .any(|keyword| normalize(keyword).contains(&query))
        })
        .collect()
}

/// Minúsculas y sin los acentos que se escriben en español (á, é, ñ…).
fn normalize(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

impl Orion {
    /// Ventana de configuración (Ctrl+, o Cmd+, en Mac, o clic derecho → Configuración…).
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.menu.open;
        let mut new = self.settings.clone();
        let menu = &mut self.menu;
        egui::Window::new("Configuración")
            .open(&mut open)
            .collapsible(false)
            .fixed_size(egui::vec2(760.0, 520.0))
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                let matches = sections_matching(&menu.filter);
                // Si la sección activa ya no coincide, salta a la primera que sí.
                if !matches.contains(&menu.section)
                    && let Some(first) = matches.first()
                {
                    menu.section = *first;
                }
                let height = ui.available_height();
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(190.0, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut menu.filter)
                                    .hint_text("Buscar…")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.add_space(6.0);
                            if matches.is_empty() {
                                ui.weak("Sin resultados");
                            }
                            for section in &matches {
                                let selected = *section == menu.section;
                                if ui.selectable_label(selected, section.title()).clicked() {
                                    menu.section = *section;
                                }
                            }
                        },
                    );
                    ui.separator();
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            if !matches.is_empty() {
                                section_content(ui, menu.section, &mut new, menu);
                            }
                        },
                    );
                });
            });
        self.menu.open = open;
        if new != self.settings {
            self.change_settings(ctx, new);
        }
    }
}

/// El título de la sección y sus ajustes, dentro de un ScrollArea que ocupa el alto disponible.
fn section_content(
    ui: &mut egui::Ui,
    section: Section,
    new: &mut Settings,
    menu: &mut SettingsMenu,
) {
    ui.heading(section.title());
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("ajustes")
        .auto_shrink([false, false])
        .show(ui, |ui| match section {
            Section::Apariencia => {
                setting_row(ui, "Tema", "Los colores de la terminal y de las ventanas", |ui| {
                    egui::ComboBox::from_id_salt("tema")
                        .selected_text(&new.theme)
                        .show_ui(ui, |ui| {
                            for t in &THEMES {
                                ui.selectable_value(&mut new.theme, t.name.to_string(), t.name);
                            }
                        });
                });
                setting_row(ui, "Fondo", "El material detrás de la terminal", |ui| {
                    let response = egui::ComboBox::from_id_salt("fondo")
                        .selected_text(&new.backdrop)
                        .show_ui(ui, |ui| {
                            for (name, _) in BACKDROPS {
                                ui.selectable_value(&mut new.backdrop, name.to_string(), name);
                            }
                        })
                        .response;
                    let _ = response.on_hover_text(
                        "Mica: tinte suave y casi sólido. Acrílico: desenfoca lo de atrás. En Windows 11 y macOS.",
                    );
                });
            }
            Section::Colores => {
                ui.weak(
                    "Hex #RRGGBB. Lo que no cambies usa el tema y se mantiene al cambiar de tema.",
                );
                ui.add_space(4.0);
                for (key, value) in new.colors.fields_mut() {
                    color_row(ui, key, value);
                    ui.separator();
                }
                ui.add_space(4.0);
                if ui.button("Restablecer todos").clicked() {
                    new.colors.clear();
                }
            }
            Section::Fuente => {
                setting_row(ui, "Tamaño", "Alto de la letra, en puntos", |ui| {
                    ui.add(egui::Slider::new(&mut new.font.size, 8.0..=48.0).step_by(1.0));
                });
                setting_row(
                    ui,
                    "Interlineado",
                    "Alto de cada línea respecto de la letra",
                    |ui| {
                        let line_height = egui::Slider::new(&mut new.font.line_height, 1.0..=2.0);
                        ui.add(line_height.step_by(0.05).fixed_decimals(2));
                    },
                );
                setting_row(ui, "Buscar fuente", "Filtra la lista por nombre", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut menu.search)
                            .hint_text("Buscar fuente…")
                            .desired_width(200.0),
                    );
                });
                setting_row(
                    ui,
                    "Mostrar también las no monoespaciadas",
                    "Incluye las de ancho variable",
                    |ui| {
                        ui.checkbox(&mut menu.show_all, "");
                    },
                );
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
                let current = menu.fonts.iter().find(|f| f.family == new.font.family);
                if current.is_some_and(|f| !f.monospace) {
                    // En una terminal todas las letras deben medir lo mismo, o la grilla se descuadra.
                    ui.add_space(4.0);
                    ui.colored_label(
                        theme().ansi[3],
                        "⚠ No es monoespaciada: las letras pueden encimarse.",
                    );
                }
                ui.add_space(4.0);
                ui.weak("Para agregar otra fuente, arrastra un archivo .ttf u .otf a la ventana.");
            }
            Section::Terminal => {
                setting_row(
                    ui,
                    "Shell por defecto",
                    "La de las pestañas nuevas",
                    |ui| {
                        let response = egui::ComboBox::from_id_salt("shell")
                            .selected_text(&new.shell)
                            .show_ui(ui, |ui| {
                                for &i in installed_shells() {
                                    let name = SHELLS[i].name;
                                    ui.selectable_value(&mut new.shell, name.to_string(), name);
                                }
                            })
                            .response;
                        let _ = response.on_hover_text(keys!(
                            "La de las pestañas nuevas (",
                            "+Shift+T",
                            ")"
                        ));
                    },
                );
                setting_row(
                    ui,
                    "Buscar actualizaciones al abrir",
                    "Revisa si hay una versión nueva en GitHub",
                    |ui| {
                        ui.checkbox(&mut new.check_updates, "");
                    },
                );
            }
            Section::Avanzado => {
                if let Some(path) = config_path() {
                    setting_row(
                        ui,
                        "Archivo de configuración",
                        "Se abre con tu editor predeterminado",
                        |ui| {
                            if ui.button("Abrir config.toml").clicked() {
                                open(Link::File {
                                    path: path.clone(),
                                    line: None,
                                    col: None,
                                });
                            }
                        },
                    );
                } else {
                    ui.weak("No encuentro la carpeta de configuración.");
                }
                ui.add_space(6.0);
                ui.weak(
                    "Los agentes ([[agent]]) y los ejecutores del orquestador ([[executor]]) se editan en ese archivo.",
                );
            }
        });
}

/// Una fila de ajuste: nombre y descripción a la izquierda, el control a la derecha.
fn setting_row(
    ui: &mut egui::Ui,
    name: &str,
    description: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(name);
            ui.weak(description);
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
    });
    ui.add_space(2.0);
    ui.separator();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn empty_query_returns_every_section() {
        assert_eq!(sections_matching(""), Section::ALL.to_vec());
        assert_eq!(sections_matching("   "), Section::ALL.to_vec());
    }

    #[test]
    fn finds_sections_by_title() {
        assert_eq!(sections_matching("apariencia"), vec![Section::Apariencia]);
        assert_eq!(sections_matching("colores"), vec![Section::Colores]);
        assert_eq!(sections_matching("terminal"), vec![Section::Terminal]);
        assert_eq!(sections_matching("avanzado"), vec![Section::Avanzado]);
    }

    #[test]
    fn finds_sections_by_keyword() {
        assert_eq!(sections_matching("letra"), vec![Section::Fuente]);
        assert_eq!(sections_matching("hex"), vec![Section::Colores]);
        assert_eq!(sections_matching("fondo"), vec![Section::Apariencia]);
        assert_eq!(sections_matching("shell"), vec![Section::Terminal]);
    }

    #[test]
    fn search_ignores_case_and_basic_accents() {
        assert_eq!(sections_matching("COLORES"), vec![Section::Colores]);
        assert_eq!(sections_matching("COLÓRES"), vec![Section::Colores]);
        assert_eq!(sections_matching("tamano"), vec![Section::Fuente]);
        assert_eq!(sections_matching("TAMAÑO"), vec![Section::Fuente]);
    }

    #[test]
    fn no_matches_gives_an_empty_list() {
        assert!(sections_matching("zzz").is_empty());
    }

    #[test]
    fn sections_have_unique_titles() {
        let titles: HashSet<&str> = Section::ALL.iter().map(|s| s.title()).collect();
        assert_eq!(titles.len(), Section::ALL.len());
    }
}
