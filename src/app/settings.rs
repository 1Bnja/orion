//! Ventana de configuración: una barra de secciones a la izquierda y los ajustes de la elegida a la derecha.

use super::MAC;
use super::Orion;
use super::chrome::{Icon, RADIUS, draw_icon, icon_button, medium};
use super::menu::{self, SettingsMenu, color_table};
use crate::backdrop::BACKDROPS;
use crate::colors::{THEMES, theme};
use crate::config::{Settings, config_path};
use crate::links::{Link, open};
use crate::pty::{SHELLS, installed_shells};
use eframe::egui;
use eframe::egui::{
    Sense, ViewportBuilder, ViewportClass, ViewportCommand, ViewportId, pos2, vec2,
};

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

    fn icon(self) -> Icon {
        match self {
            Self::Apariencia => Icon::Appearance,
            Self::Colores => Icon::Colors,
            Self::Fuente => Icon::Font,
            Self::Terminal => Icon::Terminal,
            Self::Avanzado => Icon::Advanced,
        }
    }

    /// Palabras que también encuentran la sección en el buscador.
    pub(crate) fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Apariencia => &[
                "tema", "fondo", "mica", "acrilico", "ventana", "escala", "zoom", "4k", "dpi",
            ],
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
    /// Settings in its own OS window (Cmd/Ctrl+,), so it can leave the terminal.
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.menu.open {
            return;
        }
        // Child window title bar follows the theme (light title on a dark bar).
        crate::backdrop::apply_chrome(!theme().is_light());

        let mut new = self.settings.clone();
        let mut still_open = true;
        let mut builder = ViewportBuilder::default()
            .with_title("Configuración — Orion")
            .with_inner_size([760.0, 520.0])
            .with_min_inner_size([560.0, 400.0])
            .with_resizable(true);
        // Own title strip: the system title was unreadable on a dark bar.
        if MAC {
            builder = builder
                .with_fullsize_content_view(true)
                .with_titlebar_shown(false)
                .with_title_shown(false);
        } else {
            builder = builder.with_decorations(false);
        }
        {
            let menu = &mut self.menu;
            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("orion-settings"),
                builder,
                |ui, class| {
                    if ui.input(|i| {
                        i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)
                    }) {
                        still_open = false;
                    }
                    let dark = !theme().is_light();
                    if menu.theme_sent != Some(dark) {
                        menu.theme_sent = Some(dark);
                        ui.ctx()
                            .send_viewport_cmd(ViewportCommand::SetTheme(if dark {
                                egui::SystemTheme::Dark
                            } else {
                                egui::SystemTheme::Light
                            }));
                    }

                    let mut body = |ui: &mut egui::Ui| {
                        settings_title_bar(ui, &mut still_open);
                        ui.add_space(8.0);
                        let matches = sections_matching(&menu.filter);
                        if !matches.contains(&menu.section)
                            && let Some(first) = matches.first()
                        {
                            menu.section = *first;
                        }
                        let height = ui.available_height();
                        ui.horizontal_top(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(200.0, height),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    search_field(ui, &mut menu.filter);
                                    ui.add_space(8.0);
                                    if matches.is_empty() {
                                        ui.weak("Sin resultados");
                                    }
                                    for section in &matches {
                                        if section_nav(ui, *section, *section == menu.section) {
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
                    };
                    if class == ViewportClass::EmbeddedWindow {
                        egui::Window::new("Configuración")
                            .collapsible(false)
                            .default_size([760.0, 520.0])
                            .show(ui.ctx(), |ui| body(ui));
                    } else {
                        egui::CentralPanel::default()
                            .frame(
                                egui::Frame::new()
                                    // Opaco: esta ventana no lleva Mica ni acrílico detrás, así que
                                    // el alfa de `surface()` dejaría ver lo que haya debajo sin desenfocar.
                                    .fill(theme().sidebar())
                                    .inner_margin(egui::Margin::symmetric(12, 8)),
                            )
                            .show(ui, body);
                    }
                },
            );
        }
        self.menu.open = still_open;
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
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 22.0), egui::Sense::hover());
        draw_icon(ui, section.icon(), rect.center(), theme().fg);
        ui.heading(section.title());
    });
    ui.add_space(6.0);
    egui::ScrollArea::vertical()
        .id_salt("ajustes")
        .auto_shrink([false, false])
        .show(ui, |ui| match section {
            Section::Apariencia => {
                menu::theme_preview(ui, new);
                ui.add_space(10.0);
                setting_row(ui, "Tema", "Los colores de la terminal y de las ventanas", |ui| {
                    egui::ComboBox::from_id_salt("tema")
                        .width(160.0)
                        .selected_text(&new.theme)
                        .show_ui(ui, |ui| {
                            for t in &THEMES {
                                ui.selectable_value(&mut new.theme, t.name.to_string(), t.name);
                            }
                            if !new.custom_themes.is_empty() {
                                ui.separator();
                                for t in &new.custom_themes {
                                    ui.selectable_value(
                                        &mut new.theme,
                                        t.name.clone(),
                                        t.name.as_str(),
                                    );
                                }
                            }
                        });
                });
                setting_row(ui, "Guardar", "Guarda los colores actuales con un nombre", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut menu.theme_name)
                            .hint_text("Nombre…")
                            .desired_width(110.0),
                    );
                    ui.add_space(4.0);
                    let save = ui
                        .add_sized(egui::vec2(28.0, 24.0), egui::Button::new(""))
                        .on_hover_text("Guardar como tema");
                    let center = save.rect.center();
                    draw_icon(ui, Icon::Save, center, theme().secondary());
                    if save.clicked() {
                        let name = menu.theme_name.clone();
                        match menu::save_as_theme(new, &name) {
                            Ok(()) => {
                                menu.theme_name.clear();
                                menu.theme_error = None;
                            }
                            Err(e) => menu.theme_error = Some(e),
                        }
                    }
                });
                if let Some(err) = &menu.theme_error {
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
                if new.custom_themes.iter().any(|t| t.name == new.theme) {
                    setting_row(ui, "Borrar tema", "Quita el tema propio seleccionado", |ui| {
                        if ui.button("Borrar").clicked() {
                            new.custom_themes.retain(|t| t.name != new.theme);
                            new.theme = THEMES[0].name.into();
                        }
                    });
                }
                setting_row(ui, "Fondo", "El material detrás de la terminal", |ui| {
                    let response = egui::ComboBox::from_id_salt("fondo")
                        .width(160.0)
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
                setting_row(ui, "Escala", "Tamaño de toda la interfaz, sobre el del sistema", |ui| {
                    // Una lista y no un slider: al arrastrar, la ventana cambiaría de tamaño bajo el mouse.
                    let percent = |scale: f32| format!("{:.0} %", scale * 100.0);
                    egui::ComboBox::from_id_salt("escala")
                        .width(160.0)
                        .selected_text(percent(new.ui_scale))
                        .show_ui(ui, |ui| {
                            for scale in [0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
                                ui.selectable_value(&mut new.ui_scale, scale, percent(scale));
                            }
                        })
                        .response
                        .on_hover_text(if MAC {
                            "También con Cmd+= y Cmd+- (Cmd+0 vuelve a 100 %)"
                        } else {
                            "También con Ctrl+= y Ctrl+- (Ctrl+0 vuelve a 100 %)"
                        });
                });
            }
            Section::Colores => {
                menu::theme_preview(ui, new);
                ui.add_space(8.0);
                ui.weak("Lo que no cambies sigue al tema.");
                ui.add_space(8.0);
                let basic = ["fg", "bg", "selection", "accent"];
                color_table(
                    ui,
                    "colores-basicos",
                    new.colors
                        .fields_mut()
                        .into_iter()
                        .filter(|(key, _)| basic.contains(key)),
                );
                ui.add_space(4.0);
                if more_colors_header(ui, menu.more_colors) {
                    menu.more_colors = !menu.more_colors;
                }
                if menu.more_colors {
                    ui.add_space(4.0);
                    color_table(
                        ui,
                        "colores-mas",
                        new.colors
                            .fields_mut()
                            .into_iter()
                            .filter(|(key, _)| !basic.contains(key)),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(0.0);
                    if ui
                        .add(egui::Button::new("Restablecer").frame(false))
                        .on_hover_text("Volver todos los colores al tema")
                        .clicked()
                    {
                        new.colors.clear();
                    }
                });
            }
            Section::Fuente => {
                font_preview(ui, new);
                ui.add_space(10.0);
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

/// Title strip of the settings window: readable label, drag to move, close on Windows.
fn settings_title_bar(ui: &mut egui::Ui, open: &mut bool) {
    const HEIGHT: f32 = 28.0;
    let width = ui.available_width();
    let (rect, drag) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    let left = if MAC { 72.0 } else { 4.0 };
    ui.painter().text(
        pos2(rect.min.x + left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Configuración",
        medium(14.0),
        theme().fg,
    );
    if !MAC {
        let close = egui::Rect::from_center_size(
            pos2(rect.max.x - 16.0, rect.center().y),
            vec2(24.0, 24.0),
        );
        if icon_button(ui, close, ui.scope_id().with("cerrar-ajustes"), Icon::Close)
            .on_hover_text("Cerrar")
            .clicked()
        {
            *open = false;
        }
    }
}

/// "Más colores" with a Tabler chevron (right when closed, down when open).
fn more_colors_header(ui: &mut egui::Ui, open: bool) -> bool {
    let height = 28.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if response.hovered() {
        ui.painter().rect_filled(rect, RADIUS, theme().hover());
    }
    let icon = if open {
        Icon::Chevron
    } else {
        Icon::ChevronRight
    };
    let color = if response.hovered() {
        theme().fg
    } else {
        theme().secondary()
    };
    draw_icon(
        ui,
        icon,
        egui::pos2(rect.min.x + 10.0, rect.center().y),
        color,
    );
    ui.painter().text(
        egui::pos2(rect.min.x + 28.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Más colores",
        egui::FontId::proportional(13.0),
        color,
    );
    response.clicked()
}

/// Search field with a Tabler icon on the left.
fn search_field(ui: &mut egui::Ui, filter: &mut String) {
    let height = 28.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let stroke = egui::Stroke::new(1.0, theme().line());
    ui.painter().rect(
        rect,
        RADIUS,
        theme().hover(),
        stroke,
        egui::StrokeKind::Inside,
    );
    draw_icon(
        ui,
        Icon::Search,
        egui::pos2(rect.min.x + 14.0, rect.center().y),
        theme().muted(),
    );
    let edit = egui::Rect::from_min_max(
        egui::pos2(rect.min.x + 28.0, rect.min.y),
        rect.max - egui::vec2(6.0, 0.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit));
    child.add(
        egui::TextEdit::singleline(filter)
            .hint_text("Buscar…")
            .frame(egui::Frame::NONE)
            .desired_width(f32::INFINITY),
    );
}

/// One section in the settings rail: icon + title, highlighted when selected.
fn section_nav(ui: &mut egui::Ui, section: Section, selected: bool) -> bool {
    let height = 32.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if selected {
        ui.painter().rect_filled(rect, RADIUS, theme().hover());
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, RADIUS, theme().hover().gamma_multiply(0.5));
    }
    let color = if selected {
        theme().fg
    } else {
        theme().secondary()
    };
    draw_icon(
        ui,
        section.icon(),
        egui::pos2(rect.min.x + 14.0, rect.center().y),
        color,
    );
    let text = egui::Rect::from_min_max(egui::pos2(rect.min.x + 32.0, rect.min.y), rect.max);
    ui.painter().text(
        text.left_center(),
        egui::Align2::LEFT_CENTER,
        section.title(),
        if selected {
            medium(13.0)
        } else {
            egui::FontId::proportional(13.0)
        },
        color,
    );
    response.clicked()
}

/// Sample line using the chosen font and size.
fn font_preview(ui: &mut egui::Ui, settings: &Settings) {
    let t = theme();
    let frame = egui::Frame::new()
        .fill(t.bg)
        .stroke(egui::Stroke::new(1.0, t.line()))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(12, 10));
    frame.show(ui, |ui| {
        let font = egui::FontId::new(settings.font.size, egui::FontFamily::Monospace);
        ui.label(
            egui::RichText::new("~ ❯ Orion — Aa Bb Cc 0123")
                .font(font)
                .color(t.fg),
        );
        ui.label(
            egui::RichText::new(format!(
                "{} · {:.0} pt · interlineado {:.2}",
                settings.font.family, settings.font.size, settings.font.line_height
            ))
            .size(11.0)
            .color(t.muted()),
        );
    });
}

/// A settings row: fixed-width label column on the left, control on the right.
fn setting_row(
    ui: &mut egui::Ui,
    name: &str,
    description: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    let full = ui.available_width();
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2((full * 0.48).clamp(180.0, 260.0), 40.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.label(egui::RichText::new(name).strong());
                ui.label(
                    egui::RichText::new(description)
                        .size(11.5)
                        .color(theme().muted()),
                );
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_min_width(160.0);
            control(ui);
        });
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(2.0);
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
