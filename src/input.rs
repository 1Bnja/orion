//! Entrada → bytes para la shell: teclado, pegado, mouse y rueda (convenciones de xterm).

use eframe::egui;

/// Traduce una tecla especial a los bytes que espera la shell (convención xterm).
/// El texto normal no pasa por aquí: llega como `Event::Text`.
/// `app_cursor`: la app pidió "modo cursor de aplicación" (ESC[?1h; vim, less): flechas como ESC O A.
pub(crate) fn key_bytes(
    key: egui::Key,
    mods: egui::Modifiers,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    use egui::Key::*;
    // Código de modificadores de xterm: 1 + Shift(1) + Alt(2) + Ctrl(4). Ej.: Ctrl+→ = ESC[1;5C.
    let m = 1 + mods.shift as u8 + 2 * mods.alt as u8 + 4 * mods.ctrl as u8;
    // Teclas que terminan en letra: flechas, Inicio/Fin, F1–F4.
    // `ss3` = forma ESC O x (cursor de aplicación, y siempre en F1–F4).
    let letter = |c: char, ss3: bool| {
        if m > 1 {
            format!("\x1b[1;{m}{c}")
        } else if ss3 {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        }
        .into_bytes()
    };
    // Teclas "número + ~": Insert, Supr, RePág, AvPág, F5–F12.
    let tilde = |n: u8| {
        if m > 1 {
            format!("\x1b[{n};{m}~")
        } else {
            format!("\x1b[{n}~")
        }
        .into_bytes()
    };
    Some(match key {
        Enter => b"\r".to_vec(), // las terminales mandan CR, no LF
        // Ctrl+Backspace = ^H (borrar palabra en PSReadLine, igual que en Windows Terminal).
        Backspace if mods.ctrl => vec![0x08],
        Backspace => vec![0x7f],
        Tab if mods.shift => b"\x1b[Z".to_vec(), // Shift+Tab: completar hacia atrás
        Tab => b"\t".to_vec(),
        Escape => vec![0x1b],
        ArrowUp => letter('A', app_cursor),
        ArrowDown => letter('B', app_cursor),
        ArrowRight => letter('C', app_cursor),
        ArrowLeft => letter('D', app_cursor),
        Home => letter('H', app_cursor),
        End => letter('F', app_cursor),
        F1 => letter('P', true),
        F2 => letter('Q', true),
        F3 => letter('R', true),
        F4 => letter('S', true),
        Insert => tilde(2),
        Delete => tilde(3),
        PageUp => tilde(5),
        PageDown => tilde(6),
        F5 => tilde(15),
        F6 => tilde(17), // los números saltan el 16 y el 22: herencia de terminales viejas
        F7 => tilde(18),
        F8 => tilde(19),
        F9 => tilde(20),
        F10 => tilde(21),
        F11 => tilde(23),
        F12 => tilde(24),
        // Letras con Ctrl y/o Alt. (AltGr no llega aquí: winit le quita Ctrl y Alt, y escribe como texto.)
        _ if mods.ctrl || mods.alt => {
            let &[c] = key.name().as_bytes() else {
                return None;
            };
            if !c.is_ascii_uppercase() {
                return None;
            }
            let c = if mods.ctrl {
                c - b'A' + 1 // Ctrl+letra = código de control: Ctrl+A = 0x01 … Ctrl+Z = 0x1A
            } else if mods.shift {
                c
            } else {
                c.to_ascii_lowercase()
            };
            // Alt+tecla = ESC antes de la tecla (atajos de readline, lazygit, etc.).
            if mods.alt { vec![0x1b, c] } else { vec![c] }
        }
        _ => return None,
    })
}

/// Bytes a enviar al pegar. Si la app activó "bracketed paste" (ESC[?2004h), el texto va entre
/// marcadores: así la shell sabe que es un pegado y no ejecuta cada línea como si apretaras Enter.
pub(crate) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        // Sin ESC dentro: un texto pegado con "ESC[201~" podría cerrar el pegado antes de tiempo.
        let clean = text.replace('\x1b', "");
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        // Sin bracketed paste, cada salto de línea es un Enter (CR).
        text.replace('\n', "\r").into_bytes()
    }
}

/// Codifica un evento de mouse para apps que lo piden (vim, btop, lazygit…).
/// `button`: 0 izq., 1 medio, 2 der., 64/65 rueda; +32 si es movimiento; +8 Alt, +16 Ctrl.
/// `col`/`row` empiezan en 0. `sgr`: la app pidió el formato moderno (ESC[?1006h).
pub(crate) fn mouse_report(
    button: u8,
    col: usize,
    row: usize,
    pressed: bool,
    sgr: bool,
) -> Vec<u8> {
    if sgr {
        // Formato SGR: texto legible, sin límite de tamaño. M = apretar, m = soltar.
        let end = if pressed { 'M' } else { 'm' };
        format!("\x1b[<{button};{};{}{end}", col + 1, row + 1).into_bytes()
    } else {
        // Formato clásico X10: cada valor es un byte con 32 sumado. Soltar se informa como botón 3.
        // ponytail: no puede pasar de la columna/fila 223; las apps modernas usan SGR.
        let button = if pressed { button } else { 3 | (button & !3) };
        let pos = |v: usize| (33 + v).min(255) as u8;
        vec![0x1b, b'[', b'M', 32 + button, pos(col), pos(row)]
    }
}

/// Convierte píxeles de rueda en líneas enteras. Lo que sobra queda en `acc` para el próximo frame:
/// un touchpad manda deltas chicos que por sí solos no llegan a una línea.
pub(crate) fn scroll_lines(acc: &mut f32, delta_px: f32, line_height: f32) -> i32 {
    *acc += delta_px / line_height;
    let lines = acc.trunc();
    *acc -= lines;
    lines as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_terminal_bytes() {
        use egui::{Key, Modifiers};
        let none = Modifiers::NONE;
        let key = |k, m| key_bytes(k, m, false).unwrap();
        assert_eq!(key(Key::Enter, none), b"\r");
        assert_eq!(key(Key::ArrowUp, none), b"\x1b[A");
        assert_eq!(key(Key::D, Modifiers::CTRL), [0x04]);
        assert_eq!(key_bytes(Key::A, none, false), None); // la 'a' llega como Event::Text
        assert_eq!(key_bytes(Key::Num1, Modifiers::CTRL, false), None);
        // Cursor de aplicación (vim, less): flechas e Inicio/Fin como ESC O x.
        assert_eq!(key_bytes(Key::ArrowUp, none, true).unwrap(), b"\x1bOA");
        assert_eq!(key_bytes(Key::Home, none, true).unwrap(), b"\x1bOH");
        // Modificadores: Ctrl+→ = ESC[1;5C, Shift+Supr = ESC[3;2~, Ctrl+F5 = ESC[15;5~.
        assert_eq!(key(Key::ArrowRight, Modifiers::CTRL), b"\x1b[1;5C");
        assert_eq!(key(Key::Delete, Modifiers::SHIFT), b"\x1b[3;2~");
        assert_eq!(key(Key::F5, Modifiers::CTRL), b"\x1b[15;5~");
        // Teclas de función e Insert.
        assert_eq!(key(Key::F1, none), b"\x1bOP");
        assert_eq!(key(Key::F12, none), b"\x1b[24~");
        assert_eq!(key(Key::Insert, none), b"\x1b[2~");
        // Alt+letra = ESC + letra; con Shift, mayúscula. Ctrl+Alt+letra = ESC + código de control.
        assert_eq!(key(Key::B, Modifiers::ALT), b"\x1bb");
        assert_eq!(key(Key::B, Modifiers::ALT | Modifiers::SHIFT), b"\x1bB");
        assert_eq!(key(Key::C, Modifiers::ALT | Modifiers::CTRL), [0x1b, 0x03]);
        assert_eq!(key(Key::Tab, Modifiers::SHIFT), b"\x1b[Z");
        assert_eq!(key(Key::Backspace, Modifiers::CTRL), [0x08]);
    }

    #[test]
    fn mouse_reports_in_sgr_and_classic_formats() {
        // SGR: clic izquierdo en la columna 5, fila 2 (base 0) → base 1 en el mensaje.
        assert_eq!(mouse_report(0, 4, 1, true, true), b"\x1b[<0;5;2M");
        assert_eq!(mouse_report(0, 4, 1, false, true), b"\x1b[<0;5;2m"); // soltar
        assert_eq!(mouse_report(64, 0, 0, true, true), b"\x1b[<64;1;1M"); // rueda arriba
        // Clásico: bytes con 32 sumado; soltar = botón 3.
        assert_eq!(
            mouse_report(0, 4, 1, true, false),
            [0x1b, b'[', b'M', 32, 37, 34]
        );
        assert_eq!(
            mouse_report(2, 4, 1, false, false),
            [0x1b, b'[', b'M', 35, 37, 34]
        );
        // Arrastre con botón izquierdo (32) + Ctrl (16): el "soltar" conserva los modificadores.
        assert_eq!(mouse_report(16, 0, 0, false, false)[3], 32 + 16 + 3);
    }

    #[test]
    fn paste_respects_bracketed_mode() {
        assert_eq!(paste_bytes("a\nb", false), b"a\rb");
        assert_eq!(paste_bytes("a\nb", true), b"\x1b[200~a\nb\x1b[201~");
        // Un ESC pegado no puede cerrar el bracket antes de tiempo.
        assert_eq!(
            paste_bytes("x\x1b[201~y", true),
            b"\x1b[200~x[201~y\x1b[201~"
        );
    }

    #[test]
    fn scroll_keeps_fractions_between_frames() {
        let mut acc = 0.0;
        assert_eq!(scroll_lines(&mut acc, 50.0, 16.0), 3); // 3.125 líneas: salen 3, sobra 0.125
        assert_eq!(scroll_lines(&mut acc, 10.0, 16.0), 0); // 0.125 + 0.625 = 0.75: todavía no
        assert_eq!(scroll_lines(&mut acc, 4.0, 16.0), 1); // 0.75 + 0.25 = 1: ahora sí
        assert_eq!(scroll_lines(&mut acc, -40.0, 16.0), -2); // hacia abajo: -2.5 → -2
    }
}
