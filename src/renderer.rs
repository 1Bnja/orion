//! Con qué dibuja Orion. Como Warp: wgpu sobre la API gráfica nativa de cada sistema (DirectX 12
//! en Windows, Metal en macOS), y OpenGL de respaldo si la nativa no arranca.
//!
//! En Windows, OpenGL frenaba el arrastre de la ventana: medido en un monitor de 144 Hz, la
//! ventana se movía 49 veces por segundo; con wgpu, 144.

use crate::backdrop::Backdrop;
use eframe::egui_wgpu::wgpu;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

/// Un motor que se puede elegir en Configuración o en config.toml (`renderer = ...`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(crate) enum Gpu {
    /// El nativo del sistema, y si no arranca, los demás en orden.
    Auto,
    DirectX12,
    Vulkan,
    Metal,
    OpenGl,
}

/// Los nombres que se escriben en config.toml, en orden.
pub(crate) const GPUS: [(&str, Gpu); 5] = [
    ("Automático", Gpu::Auto),
    ("DirectX 12", Gpu::DirectX12),
    ("Vulkan", Gpu::Vulkan),
    ("Metal", Gpu::Metal),
    ("OpenGL", Gpu::OpenGl),
];

impl Gpu {
    pub(crate) fn name(self) -> &'static str {
        GPUS.iter().find(|g| g.1 == self).map_or("", |g| g.0)
    }

    /// Si existe en este sistema (Metal solo en macOS; DirectX 12 y Vulkan solo en Windows).
    pub(crate) fn available(self) -> bool {
        match self {
            Gpu::Auto | Gpu::OpenGl => true,
            Gpu::DirectX12 | Gpu::Vulkan => cfg!(windows),
            Gpu::Metal => cfg!(target_os = "macos"),
        }
    }

    /// Si con este motor se ve el material de la ventana (Mica, acrílico, vibrancy).
    /// Con DirectX 12 sí, porque Orion lo usa a través de DirectComposition (`options`).
    /// Vulkan en Windows solo presenta opaco.
    fn see_through(self) -> bool {
        self != Gpu::Vulkan
    }

    /// Lo que entiende wgpu; None = OpenGL (y Automático, que no es un motor concreto).
    fn backends(self) -> Option<wgpu::Backends> {
        match self {
            Gpu::DirectX12 => Some(wgpu::Backends::DX12),
            Gpu::Vulkan => Some(wgpu::Backends::VULKAN),
            Gpu::Metal => Some(wgpu::Backends::METAL),
            Gpu::Auto | Gpu::OpenGl => None,
        }
    }
}

/// El motor con ese nombre, sin importar mayúsculas ni tildes ("directx12", "dx12", "opengl", "auto").
pub(crate) fn find_gpu(name: &str) -> Option<Gpu> {
    let fold = |s: &str| {
        s.trim()
            .to_lowercase()
            .replace('á', "a")
            .replace([' ', '-', '_'], "")
    };
    let wanted = fold(name);
    let alias = match wanted.as_str() {
        "auto" | "automatic" => Some(Gpu::Auto),
        "dx12" | "d3d12" => Some(Gpu::DirectX12),
        "gl" => Some(Gpu::OpenGl),
        _ => None,
    };
    alias.or_else(|| GPUS.iter().find(|g| fold(g.0) == wanted).map(|g| g.1))
}

/// En qué orden probar los motores. Automático: el nativo primero. En macOS, con un material
/// transparente se empieza por OpenGL: es el que se sabe que lo muestra bien detrás del texto
/// (con Metal no está probado). Siempre termina en OpenGL, que funciona en casi cualquier equipo.
pub(crate) fn attempts(choice: Gpu, backdrop: Backdrop) -> Vec<Gpu> {
    let mut order = match choice {
        Gpu::Auto if cfg!(target_os = "macos") && backdrop != Backdrop::None => {
            vec![Gpu::OpenGl, Gpu::Metal]
        }
        Gpu::Auto if cfg!(target_os = "macos") => vec![Gpu::Metal],
        Gpu::Auto => vec![Gpu::DirectX12, Gpu::Vulkan],
        chosen => vec![chosen],
    };
    if !order.contains(&Gpu::OpenGl) {
        order.push(Gpu::OpenGl);
    }
    order.retain(|g| g.available());
    // Sin duplicados en cualquier posición: `dedup` solo quita los consecutivos, y en macOS con
    // fondo transparente la lista ya empieza por OpenGL.
    let mut unique = Vec::with_capacity(order.len());
    for gpu in order {
        if !unique.contains(&gpu) {
            unique.push(gpu);
        }
    }
    unique
}

/// Las opciones de eframe para dibujar con `gpu`.
pub(crate) fn options(gpu: Gpu) -> (eframe::Renderer, eframe::egui_wgpu::WgpuConfiguration) {
    use eframe::egui_wgpu::{SurfaceConfig, WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    let Some(backends) = gpu.backends() else {
        return (eframe::Renderer::Glow, WgpuConfiguration::default());
    };
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = backends;
    // DirectComposition en vez del HWND: es lo que deja ver Mica y acrílico detrás de Orion.
    // `with_env` deja probar la otra con WGPU_DX12_PRESENTATION_SYSTEM=hwnd.
    setup
        .instance_descriptor
        .backend_options
        .dx12
        .presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual.with_env();
    let config = WgpuConfiguration {
        // Un frame en cola, no dos: lo que escribes aparece antes. Orion dibuja poco por frame.
        surface: SurfaceConfig::LOW_LATENCY,
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..Default::default()
    };
    (eframe::Renderer::Wgpu, config)
}

/// El motor con el que arrancó la ventana.
static ACTIVE: AtomicU8 = AtomicU8::new(Gpu::OpenGl as u8);
/// La tarjeta gráfica que eligió wgpu ("NVIDIA GeForce RTX 5070"), para mostrarla.
static ADAPTER: Mutex<String> = Mutex::new(String::new());

pub(crate) fn set_active(gpu: Gpu) {
    ACTIVE.store(gpu as u8, Ordering::Relaxed);
}

pub(crate) fn active() -> Gpu {
    let n = ACTIVE.load(Ordering::Relaxed);
    GPUS.iter()
        .map(|g| g.1)
        .find(|&g| g as u8 == n)
        .unwrap_or(Gpu::OpenGl)
}

/// Anota la tarjeta en uso, al crear la app.
pub(crate) fn note_adapter(cc: &eframe::CreationContext<'_>) {
    if let Some(state) = &cc.wgpu_render_state {
        *ADAPTER.lock().unwrap_or_else(|e| e.into_inner()) = state.adapter.get_info().name;
    }
}

/// "DirectX 12 · NVIDIA GeForce RTX 5070", o solo el motor.
pub(crate) fn describe() -> String {
    let adapter = ADAPTER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match adapter.is_empty() {
        true => active().name().to_string(),
        false => format!("{} · {adapter}", active().name()),
    }
}

/// El material que se puede mostrar con el motor en uso: con Vulkan, ninguno.
pub(crate) fn shown_backdrop(wanted: Backdrop) -> Backdrop {
    if active().see_through() {
        wanted
    } else {
        Backdrop::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_aliases_are_found() {
        assert_eq!(find_gpu("automatico"), Some(Gpu::Auto));
        assert_eq!(find_gpu("DirectX 12"), Some(Gpu::DirectX12));
        assert_eq!(find_gpu("dx12"), Some(Gpu::DirectX12));
        assert_eq!(find_gpu("opengl"), Some(Gpu::OpenGl));
        assert_eq!(find_gpu("metal"), Some(Gpu::Metal));
        assert_eq!(find_gpu("vidrio"), None);
        for (name, gpu) in GPUS {
            assert_eq!(gpu.name(), name);
        }
    }

    #[test]
    fn the_native_api_goes_first_and_opengl_last() {
        let auto = attempts(Gpu::Auto, Backdrop::None);
        assert_eq!(auto.last(), Some(&Gpu::OpenGl));
        if cfg!(windows) {
            assert_eq!(auto, [Gpu::DirectX12, Gpu::Vulkan, Gpu::OpenGl]);
            assert_eq!(attempts(Gpu::Auto, Backdrop::Mica)[0], Gpu::DirectX12);
            assert_eq!(
                attempts(Gpu::Vulkan, Backdrop::None),
                [Gpu::Vulkan, Gpu::OpenGl]
            );
            // Metal no existe en Windows: queda OpenGL.
            assert_eq!(attempts(Gpu::Metal, Backdrop::None), [Gpu::OpenGl]);
        }
        if cfg!(target_os = "macos") {
            assert_eq!(auto, [Gpu::Metal, Gpu::OpenGl]);
            assert_eq!(
                attempts(Gpu::Auto, Backdrop::Mica),
                [Gpu::OpenGl, Gpu::Metal]
            );
            assert_eq!(attempts(Gpu::DirectX12, Backdrop::None), [Gpu::OpenGl]);
        }
        assert_eq!(attempts(Gpu::OpenGl, Backdrop::None), [Gpu::OpenGl]);
    }
}
