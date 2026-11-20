//! macOS: las opciones de Orion van en la barra de menú del sistema (la de la manzana), no en la
//! ventana. Los atajos del menú (Cmd+Shift+T, Cmd+,…) los atrapa macOS antes que la ventana:
//! por eso en Mac `shortcuts` no los repite.

use eframe::egui;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{AboutMetadata, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::sync::mpsc::{Receiver, channel};

/// Lo que se eligió en el menú.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Command {
    Settings,
    CheckUpdates,
    Quit,
    NewTab,
    SplitRight,
    SplitDown,
    ClosePane,
    Palette,
    NextTheme,
    FontBigger,
    FontSmaller,
}

/// Atajo de un ítem: Cmd (+ Shift si el `bool` es true) + la tecla.
type Keys = Option<(bool, Code)>;

/// Cada ítem propio del menú: su id, su texto, su atajo y su comando.
const ITEMS: [(&str, &str, Keys, Command); 11] = [
    (
        "settings",
        "Configuración…",
        Some((false, Code::Comma)),
        Command::Settings,
    ),
    (
        "updates",
        "Buscar actualizaciones…",
        None,
        Command::CheckUpdates,
    ),
    (
        "quit",
        "Salir de Orion",
        Some((false, Code::KeyQ)),
        Command::Quit,
    ),
    (
        "new-tab",
        "Nueva pestaña",
        Some((true, Code::KeyT)),
        Command::NewTab,
    ),
    (
        "split-right",
        "Dividir a la derecha",
        Some((true, Code::KeyD)),
        Command::SplitRight,
    ),
    (
        "split-down",
        "Dividir hacia abajo",
        Some((true, Code::KeyE)),
        Command::SplitDown,
    ),
    (
        "close-pane",
        "Cerrar panel",
        Some((true, Code::KeyW)),
        Command::ClosePane,
    ),
    (
        "palette",
        "Paleta de comandos",
        Some((true, Code::KeyP)),
        Command::Palette,
    ),
    ("theme", "Cambiar de tema", None, Command::NextTheme),
    ("font-bigger", "Agrandar letra", None, Command::FontBigger),
    ("font-smaller", "Achicar letra", None, Command::FontSmaller),
];

/// El comando de un id del menú (None si es de un ítem de macOS, como Minimizar).
fn command_of(id: &str) -> Option<Command> {
    ITEMS.iter().find(|item| item.0 == id).map(|item| item.3)
}

pub(super) struct MacMenu {
    /// macOS no se queda con el menú: si se suelta, la barra queda vacía.
    _menu: Menu,
    events: Receiver<Command>,
}

impl MacMenu {
    /// Arma la barra de menú y la instala. Va cuando la ventana ya existe (en `Orion::new`):
    /// así reemplaza el menú mínimo que pone winit.
    pub(super) fn install(ctx: &egui::Context) -> Self {
        let items: Vec<MenuItem> = ITEMS
            .iter()
            .map(|&(id, text, keys, _)| {
                let accelerator = keys.map(|(shift, code)| {
                    let mods = if shift {
                        Modifiers::META | Modifiers::SHIFT
                    } else {
                        Modifiers::META
                    };
                    Accelerator::new(mods, code)
                });
                MenuItem::with_id(id, text, true, accelerator)
            })
            .collect();
        let item = |id: &str| -> &dyn IsMenuItem {
            items.iter().find(|i| i.id() == id).expect("ítem del menú")
        };
        let separator = PredefinedMenuItem::separator();
        let about = PredefinedMenuItem::about(
            Some("Acerca de Orion"),
            Some(AboutMetadata {
                name: Some("Orion".into()),
                version: Some(crate::update::CURRENT.into()),
                ..Default::default()
            }),
        );
        let (services, hide, hide_others, show_all) = (
            PredefinedMenuItem::services(Some("Servicios")),
            PredefinedMenuItem::hide(Some("Ocultar Orion")),
            PredefinedMenuItem::hide_others(Some("Ocultar otros")),
            PredefinedMenuItem::show_all(Some("Mostrar todo")),
        );
        let (minimize, fullscreen, front) = (
            PredefinedMenuItem::minimize(Some("Minimizar")),
            PredefinedMenuItem::fullscreen(Some("Pantalla completa")),
            PredefinedMenuItem::bring_all_to_front(Some("Traer todo al frente")),
        );
        let submenu = |text: &str, items: &[&dyn IsMenuItem]| {
            Submenu::with_items(text, true, items).expect("submenú")
        };
        // El primero es el de la app: macOS le pone el nombre de la app ("Orion").
        let app = submenu(
            "Orion",
            &[
                &about,
                &separator,
                item("settings"),
                item("updates"),
                &separator,
                &services,
                &separator,
                &hide,
                &hide_others,
                &show_all,
                &separator,
                item("quit"),
            ],
        );
        let shell = submenu(
            "Shell",
            &[
                item("new-tab"),
                &separator,
                item("split-right"),
                item("split-down"),
                item("close-pane"),
            ],
        );
        let view = submenu(
            "Ver",
            &[
                item("palette"),
                &separator,
                item("theme"),
                item("font-bigger"),
                item("font-smaller"),
            ],
        );
        let window = submenu("Ventana", &[&minimize, &fullscreen, &separator, &front]);
        let menu = Menu::with_items(&[&app, &shell, &view, &window]).expect("menú");
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();

        // macOS avisa desde su bucle de eventos: se guarda el comando y se pide un frame para atenderlo.
        let (tx, events) = channel();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(command) = command_of(&event.id.0) {
                let _ = tx.send(command);
                ctx.request_repaint();
            }
        }));
        Self {
            _menu: menu,
            events,
        }
    }

    /// El próximo comando elegido en el menú, si hay.
    pub(super) fn next(&self) -> Option<Command> {
        self.events.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_id_maps_to_its_own_command() {
        for (id, _, _, command) in ITEMS {
            assert_eq!(command_of(id), Some(command));
        }
        assert_eq!(command_of("minimizar"), None);
    }
}
