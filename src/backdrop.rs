//! Fondo de Windows 11 detrás de la ventana: Mica (toma un tinte suave del fondo de pantalla)
//! o acrílico (desenfoca lo que hay detrás). Lo pinta Windows (DWM, el compositor de ventanas):
//! nosotros le pedimos el material y dejamos el fondo de la terminal semitransparente para que se vea.

use std::ffi::c_void;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Backdrop {
    None,
    Mica,
    Acrylic,
}

/// Los nombres que se escriben en config.toml, en orden.
pub(crate) const BACKDROPS: [(&str, Backdrop); 3] = [
    ("Ninguno", Backdrop::None),
    ("Mica", Backdrop::Mica),
    ("Acrílico", Backdrop::Acrylic),
];

/// El fondo con ese nombre (sin importar mayúsculas ni la tilde de "acrílico").
pub(crate) fn find_backdrop(name: &str) -> Option<Backdrop> {
    let name = name.trim().to_lowercase().replace('í', "i");
    BACKDROPS
        .iter()
        .find(|(n, _)| n.to_lowercase().replace('í', "i") == name)
        .map(|&(_, b)| b)
}

impl Backdrop {
    /// Opacidad del fondo de la terminal (0-255) con este material: lo que falta deja ver el material.
    /// ponytail: fija; si alguien la quiere más o menos transparente, va a config.toml.
    pub(crate) fn alpha(self) -> u8 {
        match self {
            Backdrop::None => 255,
            Backdrop::Mica => 200,
            Backdrop::Acrylic => 170,
        }
    }
}

#[cfg(windows)]
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const c_void, size: u32) -> i32;
}

// Atributos de DwmSetWindowAttribute (dwmapi.h).
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
// Valores de DWMWA_SYSTEMBACKDROP_TYPE: 1 = ninguno, 2 = Mica, 3 = acrílico.
const DWMSBT_NONE: u32 = 1;
const DWMSBT_MAINWINDOW: u32 = 2;
const DWMSBT_TRANSIENTWINDOW: u32 = 3;

/// Le pide a Windows el material `backdrop` para la ventana `hwnd`, y la barra de título
/// clara u oscura según el tema. En Windows 10 o un Windows 11 viejo no hace nada (DWM ignora el atributo).
#[cfg(windows)]
pub(crate) fn apply(hwnd: isize, backdrop: Backdrop, dark: bool) {
    let set = |attribute: u32, value: u32| {
        // SAFETY: `value` vive durante la llamada y DWM solo lee `size` bytes.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute,
                (&value as *const u32).cast(),
                size_of::<u32>() as u32,
            )
        };
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, dark as u32);
    let kind = match backdrop {
        Backdrop::None => DWMSBT_NONE,
        Backdrop::Mica => DWMSBT_MAINWINDOW,
        Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
    };
    set(DWMWA_SYSTEMBACKDROP_TYPE, kind);
}

#[cfg(not(windows))]
pub(crate) fn apply(_hwnd: isize, _backdrop: Backdrop, _dark: bool) {}

/// El HWND (identificador de ventana de Windows) de lo que nos pase eframe.
pub(crate) fn hwnd_of(window: &impl raw_window_handle::HasWindowHandle) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backdrop_names() {
        assert_eq!(find_backdrop("mica"), Some(Backdrop::Mica));
        assert_eq!(find_backdrop("Acrilico"), Some(Backdrop::Acrylic)); // sin tilde también
        assert_eq!(find_backdrop("ninguno"), Some(Backdrop::None));
        assert_eq!(find_backdrop("vidrio"), None);
        assert_eq!(Backdrop::None.alpha(), 255); // sin material, el fondo es opaco como siempre
    }
}
