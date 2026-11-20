//! Orion: una terminal simple para Windows 11, escrita en Rust.
//!
//! Recorrido de los datos:
//!   teclado/mouse → `input` → shell (en un `pty`) → bytes → parser VT → `Term` (la grilla)
//!   → `app::render` la dibuja con egui, usando `colors`, `fonts` y la geometría de `grid`.
//!   Antes del parser, `blocks` saca las marcas OSC 133 y arma un bloque por comando.
//!   Cada pestaña (`app::session`) tiene su propia shell, grilla y bloques.

// En release, Orion es una app de ventana: sin esto, Windows abre una consola negra detrás.
// En debug la dejamos: ahí se ven los mensajes de `eprintln!`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agents;
mod app;
mod backdrop;
mod blocks;
mod colors;
mod config;
mod fonts;
mod grid;
mod icon;
mod input;
mod links;
mod pty;
mod update;

fn main() -> Result<(), pty::Error> {
    // Tamaño inicial; se corrige en el primer frame, cuando ya sabemos cuánto mide la ventana.
    let grid = (80, 24);

    // Una app de ventana no tiene consola: si Orion se cae, el motivo queda en %APPDATA%\orion\crash.log.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(dir) = config::config_path().and_then(|p| Some(p.parent()?.to_path_buf())) {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("crash.log"), format!("{info}\n"));
        }
        default_hook(info);
    }));

    update::clean_up(); // borra el .exe viejo si venimos de una actualización
    let settings = config::load_settings();
    // Para saber qué se aplicó: útil al probar fuentes nuevas.
    if let Some(path) = config::config_path() {
        eprintln!("orion: config {}", path.display());
    }

    let viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Orion")
        .with_inner_size([900.0, 560.0])
        // El de la barra de tareas y la esquina de la ventana (el del .exe lo pone build.rs).
        .with_icon(eframe::egui::IconData {
            rgba: icon::render(64),
            width: 64,
            height: 64,
        })
        // Transparente: deja ver Mica o el acrílico (ver `backdrop`).
        .with_transparent(true);
    // Sin la barra del sistema: Orion dibuja la suya (ver `app::titlebar`).
    let viewport = if cfg!(target_os = "macos") {
        // En Mac la ventana conserva sus bordes (esquinas redondeadas, los círculos nativos,
        // el verde a pantalla completa) y el contenido de Orion ocupa también la barra de título.
        viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
    } else {
        viewport.with_decorations(false)
    };
    let options = eframe::NativeOptions {
        viewport,
        // OpenGL: en Windows respeta la transparencia de la ventana; el motor por defecto (wgpu) no siempre.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Orion",
        options,
        Box::new(|cc| Ok(Box::new(app::Orion::new(cc, settings, grid)?))),
    )
    // Con OpenGL, el error de eframe no se puede pasar entre hilos (no es `Send`): lo pasamos a texto.
    .map_err(|e| e.to_string())?;
    Ok(())
}
