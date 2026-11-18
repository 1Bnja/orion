//! Orion: una terminal simple para Windows 11, escrita en Rust.
//!
//! Recorrido de los datos:
//!   teclado/mouse → `input` → shell (en un `pty`) → bytes → parser VT → `Term` (la grilla)
//!   → `app::render` la dibuja con egui, usando `colors`, `fonts` y la geometría de `grid`.
//!   Antes del parser, `blocks` saca las marcas OSC 133 y arma un bloque por comando.
//!   Cada pestaña (`app::session`) tiene su propia shell, grilla y bloques.

mod app;
mod blocks;
mod colors;
mod config;
mod fonts;
mod grid;
mod input;
mod pty;

fn main() -> Result<(), pty::Error> {
    // Tamaño inicial; se corrige en el primer frame, cuando ya sabemos cuánto mide la ventana.
    let grid = (80, 24);

    let settings = config::load_settings();
    // Para saber qué se aplicó: útil al probar fuentes nuevas.
    if let Some(path) = config::config_path() {
        eprintln!("orion: config {}", path.display());
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Orion")
            .with_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Orion",
        options,
        Box::new(|cc| Ok(Box::new(app::Orion::new(cc, settings, grid)?))),
    )?;
    Ok(())
}
