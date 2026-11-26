//! Material detrás de la ventana: Mica (un tinte suave y casi sólido) o acrílico (desenfoca lo que
//! hay detrás). Lo pinta el sistema: en Windows 11, DWM (el compositor de ventanas); en macOS, un
//! `NSVisualEffectView` (vibrancy). Nosotros pedimos el material y dejamos el fondo de la terminal
//! semitransparente para que se vea.

use raw_window_handle::RawWindowHandle;

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
    /// En macOS el material deja pasar más luz que en Windows, así que el fondo va más opaco
    /// para que el texto siga legible.
    pub(crate) fn alpha(self) -> u8 {
        let mac = cfg!(target_os = "macos");
        match self {
            Backdrop::None => 255,
            Backdrop::Mica if mac => 230,
            Backdrop::Mica => 200,
            Backdrop::Acrylic if mac => 205,
            Backdrop::Acrylic => 170,
        }
    }
}

#[cfg(windows)]
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(
        hwnd: isize,
        attribute: u32,
        value: *const std::ffi::c_void,
        size: u32,
    ) -> i32;
}

/// Le pide a Windows el material `backdrop` para la ventana `hwnd`, y la barra de título
/// clara u oscura según el tema. En Windows 10 o un Windows 11 viejo no hace nada (DWM ignora el atributo).
#[cfg(windows)]
pub(crate) fn apply(window: Window, backdrop: Backdrop, dark: bool) {
    let RawWindowHandle::Win32(h) = window.0 else {
        return;
    };
    let hwnd = h.hwnd.get();
    // Atributos de DwmSetWindowAttribute (dwmapi.h).
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    // Valores de DWMWA_SYSTEMBACKDROP_TYPE: 1 = ninguno, 2 = Mica, 3 = acrílico.
    const DWMSBT_NONE: u32 = 1;
    const DWMSBT_MAINWINDOW: u32 = 2;
    const DWMSBT_TRANSIENTWINDOW: u32 = 3;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;

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
    // Sin la barra de Windows (Orion dibuja la suya), las esquinas se pedirían rectas: las queremos redondas.
    set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
}

/// Le pone a la ventana de macOS un `NSVisualEffectView` detrás del contenido de Orion, con el
/// material de `backdrop`, y la apariencia clara u oscura según el tema (también para los botones
/// de la barra). La vista se crea una sola vez; con `Backdrop::None` se oculta.
#[cfg(target_os = "macos")]
pub(crate) fn apply(window: Window, backdrop: Backdrop, dark: bool) {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAutoresizingMaskOptions, NSColor, NSView, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
    };
    use std::cell::RefCell;

    thread_local! {
        /// La vista de efecto ya creada, para reusarla al cambiar de fondo o de tema.
        static EFFECT: RefCell<Option<Retained<NSVisualEffectView>>> = const { RefCell::new(None) };
    }

    let RawWindowHandle::AppKit(h) = window.0 else {
        return;
    };
    // AppKit solo se toca desde el hilo principal; eframe nos llama desde ahí.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // SAFETY: `ns_view` es la vista de contenido que creó winit; vive mientras viva la ventana,
    // y la ventana vive mientras vive la app. Estamos en el hilo principal (ver `mtm`).
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    let Some(ns_window) = view.window() else {
        return;
    };
    // SAFETY: leer el padre de una vista válida desde el hilo principal.
    let Some(frame) = (unsafe { view.superview() }) else {
        return;
    };

    let name = if dark {
        // SAFETY: constante de AppKit, siempre inicializada mientras AppKit esté cargado.
        unsafe { NSAppearanceNameDarkAqua }
    } else {
        // SAFETY: ídem.
        unsafe { NSAppearanceNameAqua }
    };
    ns_window.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
    // Sin fondo propio: lo que no pinta Orion deja ver el material (o el escritorio, si no hay).
    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));

    let material = match backdrop {
        Backdrop::None => None,
        // Mica: casi sólido, con un tinte suave de lo que hay detrás.
        Backdrop::Mica => Some(NSVisualEffectMaterial::UnderWindowBackground),
        // Acrílico: más translúcido, con el desenfoque bien visible.
        Backdrop::Acrylic => Some(NSVisualEffectMaterial::HUDWindow),
    };

    EFFECT.with_borrow_mut(|effect| {
        let Some(material) = material else {
            if let Some(effect) = effect {
                effect.setHidden(true);
            }
            return;
        };
        let effect = effect.get_or_insert_with(|| {
            let effect = NSVisualEffectView::initWithFrame(mtm.alloc(), frame.bounds());
            effect.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            // Desenfoca lo que está detrás de la ventana (no otras vistas de Orion).
            effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            // Siempre activo: si no, se apaga (queda gris liso) al perder el foco.
            effect.setState(NSVisualEffectState::Active);
            // Hermano de la vista de Orion, debajo: así OpenGL dibuja encima y no lo tapa.
            frame.addSubview_positioned_relativeTo(
                &effect,
                NSWindowOrderingMode::Below,
                Some(view),
            );
            effect
        });
        effect.setMaterial(material);
        effect.setHidden(false);
    });
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn apply(_window: Window, _backdrop: Backdrop, _dark: bool) {}

/// Title bar / traffic-light chrome for every OS window (main + settings).
/// Call when a child window opens so it matches the theme (light title on dark bar).
pub(crate) fn apply_chrome(dark: bool) {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{
            NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua,
            NSAppearanceNameDarkAqua, NSApplication,
        };
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let name = if dark {
            // SAFETY: AppKit constant while AppKit is loaded.
            unsafe { NSAppearanceNameDarkAqua }
        } else {
            // SAFETY: same.
            unsafe { NSAppearanceNameAqua }
        };
        let appearance = NSAppearance::appearanceNamed(name);
        let app = NSApplication::sharedApplication(mtm);
        for window in app.windows().iter() {
            window.setAppearance(appearance.as_deref());
        }
    }
    #[cfg(windows)]
    {
        let _ = dark; // Child windows get dark mode via the custom title bar instead.
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = dark;
    }
}

/// La ventana nativa de lo que nos pase eframe: el HWND en Windows, la `NSView` en macOS.
/// En Linux no hay material que pedir: `of` da None y el campo no se usa.
#[derive(Clone, Copy)]
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) struct Window(RawWindowHandle);

impl Window {
    pub(crate) fn of(window: &impl raw_window_handle::HasWindowHandle) -> Option<Self> {
        match window.window_handle().ok()?.as_raw() {
            raw @ (RawWindowHandle::Win32(_) | RawWindowHandle::AppKit(_)) => Some(Self(raw)),
            _ => None,
        }
    }
}

#[cfg(windows)]
#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetStockObject(kind: i32) -> isize;
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetClientRect(hwnd: isize, rect: *mut [i32; 4]) -> i32;
    fn GetDC(hwnd: isize) -> isize;
    fn ReleaseDC(hwnd: isize, dc: isize) -> i32;
    fn FillRect(dc: isize, rect: *const [i32; 4], brush: isize) -> i32;
}

/// Pinta de negro la capa GDI de la ventana entera. Hay que llamarla cada vez que la ventana
/// cambia de tamaño.
///
/// Con DirectX 12, Orion dibuja por DirectComposition, encima de esa capa, que nadie pinta. Al
/// agrandar la ventana, Windows no limpia la parte nueva de la capa, y donde quedó el contenido
/// viejo se veía un rectángulo más claro del tamaño anterior. Como winit deja la ventana con
/// blur-behind, el negro de GDI cuenta como transparente: así se ve el material parejo detrás.
///
/// Solo con DirectX 12. Con OpenGL esa capa es donde se dibuja, y pintarla taparía el contenido.
/// ponytail: con WGPU_DX12_PRESENTATION_SYSTEM=hwnd (solo para pruebas) no usa DirectComposition y
/// esto sobra; si alguien lo usa de verdad, mirar también ese modo.
#[cfg(windows)]
pub(crate) fn clear_gdi_layer(window: Window) {
    let RawWindowHandle::Win32(h) = window.0 else {
        return;
    };
    let hwnd = h.hwnd.get();
    const BLACK_BRUSH: i32 = 4;
    // SAFETY: el DC se pide y se devuelve aquí mismo, y `rect` vive durante las llamadas.
    unsafe {
        let mut rect = [0; 4];
        if GetClientRect(hwnd, &mut rect) == 0 {
            return;
        }
        let dc = GetDC(hwnd);
        if dc == 0 {
            return;
        }
        FillRect(dc, &rect, GetStockObject(BLACK_BRUSH));
        ReleaseDC(hwnd, dc);
    }
}

#[cfg(not(windows))]
pub(crate) fn clear_gdi_layer(_window: Window) {}

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
        // Con material, más transparente que sin él, y el acrílico más que Mica.
        assert!(Backdrop::Acrylic.alpha() < Backdrop::Mica.alpha());
        assert!(Backdrop::Mica.alpha() < 255);
    }
}
