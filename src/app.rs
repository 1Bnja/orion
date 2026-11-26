//! La app: las pestañas, la configuración y lo que pasa en cada frame.
//!
//! Cada pestaña es una terminal o un grafo de commits (`graph`). Una terminal tiene uno o más
//! paneles (splits) y cada panel es una `Session` (su shell, su grilla, sus bloques). `session`, `panes`, `render` y `menu` son submódulos: al ser hijos de `app`,
//! pueden usar los campos privados de `Orion`.

/// En macOS los atajos de Orion van con Cmd y Control queda para la terminal (Ctrl+C interrumpe).
const MAC: bool = cfg!(target_os = "macos");

/// Nombre de un atajo para tooltips y la paleta: `keys!("+Shift+P")` es "Ctrl+Shift+P" en
/// Windows y "Cmd+Shift+P" en Mac. Con texto antes y después:
/// `keys!("Paleta   ", "+Shift+P", "")` es "Paleta   Ctrl+Shift+P".
macro_rules! keys {
    ($rest:literal) => {
        keys!("", $rest, "")
    };
    ($before:literal, $rest:literal, $after:literal) => {
        if cfg!(target_os = "macos") {
            concat!($before, "Cmd", $rest, $after)
        } else {
            concat!($before, "Ctrl", $rest, $after)
        }
    };
}

mod chrome;
mod graph;
mod graph_actions;
mod graph_panels;
mod home;
#[cfg(target_os = "macos")]
mod macmenu;
mod menu;
mod orchestrate;
mod overlay;
mod palette;
mod panes;
mod project;
mod render;
mod review;
mod session;
mod settings;
mod settings_agents;
mod sidebar;
mod titlebar;

use crate::blocks::Status;
use crate::colors::{surface, theme};
use crate::config::{Settings, UI_SCALE, config_modified, reload_settings};
use crate::fonts::{apply_font, is_font_file};
use crate::logo::Logo;
use crate::pty::{Error, find_shell, usable_shell};
use crate::update::{self, State as UpdateState, Updater};
use alacritty_terminal::term::TermMode;
use eframe::egui;
use menu::SettingsMenu;
use palette::Palette;
use panes::Split;
use project::Project;
use session::{AgentStatus, AgentWatch, Session};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

/// Lo que muestra una pestaña.
enum Content {
    Terminal(Split<Session>),
    /// El grafo de commits de `Tab::repo`: sin shell, sin splits y sin teclado para una terminal.
    Graph(Box<graph::GraphTab>),
}

/// Una pestaña: sus paneles y cuál tiene el foco.
struct Tab {
    content: Content,
    /// El `id` del panel que recibe el teclado (en una pestaña Grafo no apunta a nada).
    focus: u64,
    /// Dónde se dibujó cada panel en el último frame (para mover el foco con Ctrl+Alt+flechas).
    rects: Vec<(u64, egui::Rect)>,
    /// Raíz del repo al que pertenece (para agrupar la barra lateral); None = terminal suelta.
    repo: Option<PathBuf>,
    /// El agente con que se abrió, para distinguir varias pestañas de un mismo proyecto.
    agent: Option<String>,
    /// Original launch command, retained for the tab logo after command blocks are cleared.
    agent_command: Option<String>,
    /// La carpeta del worktree si la pestaña nació en uno (para no limpiarlo mientras se usa).
    worktree: Option<PathBuf>,
    /// El `id` del panel donde Orion escribió el agente (con splits el foco cambia); su
    /// `Session::agent` dice cómo va. None = pestaña que no es de agente.
    agent_run: Option<u64>,
    /// Ya se avisó que el agente terminó con error (para no repetir el aviso cada frame).
    agent_notified: bool,
    /// El cwd con que se calculó `repo` por última vez (ver `repo_to_check`).
    repo_cwd: Option<PathBuf>,
    /// El repo de ese cwd, que un hilo le está preguntando a git.
    repo_check: Option<Receiver<Option<PathBuf>>>,
}

impl Tab {
    fn new(session: Session) -> Self {
        let focus = session.id;
        Self::with(Content::Terminal(Split::new(session)), focus)
    }

    /// Una pestaña Grafo del repo `repo`.
    fn graph(repo: PathBuf) -> Self {
        let graph = Box::new(graph::GraphTab::new(repo.clone()));
        let mut tab = Self::with(Content::Graph(graph), 0);
        tab.repo = Some(repo);
        tab
    }

    fn with(content: Content, focus: u64) -> Self {
        Self {
            repo: None,
            agent: None,
            agent_command: None,
            worktree: None,
            agent_run: None,
            agent_notified: false,
            repo_cwd: None,
            repo_check: None,
            focus,
            content,
            rects: Vec::new(),
        }
    }

    /// Los paneles, si es una terminal.
    fn panes(&self) -> Option<&Split<Session>> {
        match &self.content {
            Content::Terminal(panes) => Some(panes),
            Content::Graph(_) => None,
        }
    }

    fn panes_mut(&mut self) -> Option<&mut Split<Session>> {
        match &mut self.content {
            Content::Terminal(panes) => Some(panes),
            Content::Graph(_) => None,
        }
    }

    /// El panel con foco; None en una pestaña Grafo.
    fn focused(&self) -> Option<&Session> {
        self.panes()?.get(self.focus)
    }

    fn focused_mut(&mut self) -> Option<&mut Session> {
        let focus = self.focus;
        self.panes_mut()?.get_mut(focus)
    }

    /// El título del panel con foco, o "Grafo".
    fn title(&self) -> String {
        self.focused()
            .map_or_else(|| "Grafo".into(), Session::title)
    }

    /// El logo del agente de esta pestaña (ver `agent_logo`).
    fn logo(&self) -> Option<Logo> {
        let session = self.focused()?;
        let running = matches!(session.activity(), Some(Status::Running(_)))
            .then(|| session.last_command())
            .flatten();
        let launched = self.agent_command.as_deref();
        agent_logo(running, launched, &session.title())
    }

    /// Cómo va el agente de esta pestaña; None si no es de agente (o su panel ya se cerró).
    pub(super) fn agent_status(&self) -> Option<AgentStatus> {
        let pane = self.agent_run?;
        Some(self.panes()?.get(pane)?.agent?.status)
    }
}

pub(crate) struct Orion {
    /// Las pestañas abiertas, de arriba abajo.
    tabs: Vec<Tab>,
    /// La pestaña que se ve.
    active: usize,
    settings: Settings,
    menu: SettingsMenu,
    palette: Palette,
    orchestrate: orchestrate::Orchestrate,
    project: Project,
    /// La ventana "Ver cambios" de una pestaña con worktree.
    review: Option<review::Review>,
    /// Proyectos guardados (projects.json).
    saved: Vec<project::Saved>,
    /// Si la ventana tenía el foco en el frame anterior (para avisar cambios a la app de la pestaña).
    focused: bool,
    /// La barra lateral de pestañas está oculta: la terminal usa todo el ancho.
    sidebar_hidden: bool,
    /// Aviso breve ("Salida copiada") y desde cuándo se muestra.
    toast: Option<(String, Instant)>,
    /// Último título que le pusimos a la ventana (para cambiarlo solo cuando hace falta).
    window_title: String,
    /// Fecha de config.toml la última vez que lo miramos, y cuándo fue eso.
    config_modified: Option<SystemTime>,
    config_checked: Instant,
    /// Some = se muestra el medidor de rendimiento.
    perf: Option<Perf>,
    /// La ventana nativa (HWND en Windows, NSView en macOS): para pedirle Mica o acrílico.
    window: Option<crate::backdrop::Window>,
    /// El tamaño de la ventana en el último frame, para saber cuándo cambió.
    window_size: Option<egui::Vec2>,
    /// Cuándo volver a limpiar la capa GDI (ver `clear_gdi_layer`).
    gdi_recheck: Option<Instant>,
    updater: Updater,
    /// Buscaste actualizaciones a mano: avisar también si está al día o si falló.
    /// (La revisión automática al abrir es silenciosa: sin internet no hay nada que molestar.)
    update_asked: bool,
    /// El ícono de Orion, para la barra de arriba.
    logo: egui::TextureHandle,
    /// Fallback grid size when no tab is open yet (from the initial window estimate).
    default_grid: (u16, u16),
    /// Mac: la barra de menú del sistema, con las opciones de Orion.
    #[cfg(target_os = "macos")]
    mac_menu: macmenu::MacMenu,
}

/// Medidor de rendimiento (paleta → "Mostrar rendimiento"): cuánto tarda cada frame y cuántos hay por segundo.
struct Perf {
    /// Tiempo de CPU por frame en ms, promediado para que el número no salte.
    avg_ms: f32,
    frames: u32,
    fps: u32,
    since: Instant,
}

impl Perf {
    fn new() -> Self {
        Self {
            avg_ms: 0.0,
            frames: 0,
            fps: 0,
            since: Instant::now(),
        }
    }

    /// Suma un frame que tardó `cpu` segundos (lo mide eframe: nuestro código + preparar los triángulos).
    fn frame(&mut self, cpu: f32) {
        self.avg_ms = self.avg_ms * 0.9 + cpu * 1000.0 * 0.1;
        self.frames += 1;
        if self.since.elapsed() >= Duration::from_secs(1) {
            (self.fps, self.frames, self.since) = (self.frames, 0, Instant::now());
        }
    }
}

/// Space between the terminal panes and the window edges, and between two panes.
const PANE_GAP: f32 = 8.0;
/// Corner radius of a terminal pane.
const PANE_RADIUS: f32 = 10.0;

/// Cada cuánto se revisa si config.toml cambió.
/// ponytail: se pregunta la fecha del archivo (stdlib); el crate `notify` avisaría al instante, sin preguntar.
const CONFIG_POLL: Duration = Duration::from_secs(1);
/// Cuánto después de un cambio de tamaño se vuelve a limpiar la capa GDI de la ventana.
/// ponytail: medido a ojo; si al abrir todavía se ve claro, subirlo.
const GDI_RECHECK: Duration = Duration::from_millis(500);

/// Algo que se pidió sobre las pestañas o los paneles en este frame.
#[derive(Clone)]
enum Action {
    /// Pestaña nueva con esa shell (índice en `pty::SHELLS`), o con la de config.toml.
    NewTab(Option<usize>),
    CloseTab(usize),
    SelectTab(usize),
    /// Dividir el panel con foco: a la derecha (true) o hacia abajo.
    Split(bool),
    ClosePane,
    /// Mover el foco al panel de al lado, ej. (1, 0) = el de la derecha.
    Focus(egui::Vec2),
    /// Abrir o cerrar la paleta de comandos.
    Palette,
    /// Mostrar u ocultar la barra lateral de pestañas.
    ToggleSidebar,
    OpenSettings,
    /// Abrir Configuración en la sección Agentes.
    AddAgent,
    /// Cambiar el tamaño de la letra en tantos puntos.
    FontSize(f32),
    /// Usar el tema con ese índice en `colors::THEMES`.
    Theme(usize),
    TogglePerf,
    /// Usar el fondo con ese índice en `backdrop::BACKDROPS`.
    Backdrop(usize),
    CheckUpdates,
    /// Bajar e instalar esa versión.
    InstallUpdate(String),
    RestartForUpdate,
    /// Abrir el agente con ese índice en `settings.agents`.
    Agent(usize),
    /// Abrir el agente preinstalado con ese índice en `agents::PROVIDERS`.
    Provider(usize),
    /// Abrir la ventana del orquestador (una tarea → varios agentes, cada uno con su modelo).
    Orchestrate,
    /// Abrir la ventana de "Nuevo proyecto": un repo y un agente, en su propia pestaña.
    NewProject,
    /// Otro agente en el repo de esa pestaña (el `bool` es si va en un worktree nuevo).
    InProject(usize, AgentChoice, bool),
    /// Una terminal sola en el repo de esa pestaña.
    TerminalIn(usize),
    /// Borrar los worktrees de Orion que ya no usa ninguna pestaña (y su rama, si se puede).
    CleanWorktrees,
    /// Reabrir el proyecto guardado con ese índice en `Orion::saved`.
    OpenSaved(usize),
    /// Quitarlo de los proyectos guardados.
    ForgetSaved(usize),
    /// Abrir "Ver cambios" de la pestaña con worktree.
    Review(usize),
    /// Abrir (o enfocar, si ya está) la pestaña Grafo del repo de esa pestaña.
    Graph(usize),
    /// Fusionar la rama de esa pestaña en la rama del repo principal.
    Merge(usize),
    /// Borrar el worktree y la rama de esa pestaña, y cerrarla.
    Discard(usize),
}

/// Un agente que se puede elegir para un proyecto: uno de los que Orion detecta solo, o un
/// perfil `[[agent]]` de config.toml.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AgentChoice {
    Provider(usize),
    Profile(usize),
}

impl AgentChoice {
    fn name(self, settings: &Settings) -> &str {
        match self {
            Self::Provider(i) => crate::agents::PROVIDERS[i].name,
            Self::Profile(i) => &settings.agents[i].name,
        }
    }

    fn logo(self, settings: &Settings) -> Logo {
        match self {
            Self::Provider(i) => crate::agents::PROVIDERS[i].logo,
            Self::Profile(i) => crate::agents::logo_for(&settings.agents[i].command),
        }
    }

    fn command(self, settings: &Settings) -> String {
        match self {
            Self::Provider(i) => crate::agents::PROVIDERS[i].command.into(),
            Self::Profile(i) => settings.agents[i].command.clone(),
        }
    }

    /// Si por defecto el proyecto nace en un git worktree y una rama nueva: cuando el repo ya tiene
    /// una pestaña abierta (`busy`) para no pisarla, o cuando el perfil lo pide.
    fn worktree(self, settings: &Settings, busy: bool) -> bool {
        match self {
            Self::Provider(_) => busy,
            Self::Profile(i) => busy || settings.agents[i].worktree,
        }
    }
}

/// El orden en que la barra lateral muestra las pestañas, dado el repo de cada una: los repos en el
/// orden en que aparecen y, al final, las terminales sueltas. Dentro de un grupo, el orden de apertura.
fn grouped_order(repos: &[Option<&PathBuf>]) -> Vec<usize> {
    let mut firsts: Vec<&PathBuf> = Vec::new();
    for r in repos.iter().flatten() {
        if !firsts.contains(r) {
            firsts.push(r);
        }
    }
    let mut order: Vec<usize> = (0..repos.len()).collect();
    order.sort_by_key(|&i| {
        repos[i]
            .and_then(|r| firsts.iter().position(|x| *x == r))
            .unwrap_or(usize::MAX)
    });
    order
}

/// El logo de una pestaña; None si es una terminal sin agente. Manda lo que está corriendo en
/// el panel con foco (`running`, por si abriste el agente a mano); si no es un agente conocido,
/// el comando con que Orion abrió la pestaña (`launched`; genérico si no lo reconoce). Lo último,
/// que el título sea el nombre de un agente: al limpiar la pantalla se pierden los bloques.
fn agent_logo(running: Option<&str>, launched: Option<&str>, title: &str) -> Option<Logo> {
    let known = |command| crate::agents::provider_for(command).map(|p| p.logo);
    let titled = || {
        let name = plain_title(title);
        let mut providers = crate::agents::PROVIDERS.iter();
        providers.find(|p| p.name == name).map(|p| p.logo)
    };
    running
        .and_then(known)
        .or_else(|| launched.map(crate::agents::logo_for))
        .or_else(titled)
}

/// El título sin el símbolo de estado que algunos agentes ponen delante (el "✳" de Claude Code
/// o un indicador giratorio): junto al logo sería un segundo ícono.
fn plain_title(title: &str) -> &str {
    let glyph = |c: char| "✳✢✶✻✽∗·✦◇\u{fe0f}".contains(c) || ('\u{2800}'..='\u{28ff}').contains(&c);
    let rest = title.trim_start_matches(glyph).trim_start();
    if rest.is_empty() { title } else { rest }
}

/// Lo que dice la fila de una pestaña: el agente y la carpeta, salvo que el título ya nombre al agente
/// (varios agentes ponen su nombre de título de la terminal).
fn tab_label(agent: Option<&str>, title: &str) -> String {
    match agent {
        Some(a) if !title.to_lowercase().contains(&a.to_lowercase()) => format!("{a} · {title}"),
        _ => title.to_string(),
    }
}

/// La carpeta en que hay que volver a buscar el repo de una pestaña, si hace falta: solo las
/// terminales sin worktree ni agente siguen su cwd, y solo cuando cambió desde la última vez
/// (`checked`), para no llamar a git cada frame.
fn repo_to_check<'a>(
    worktree: bool,
    agent: bool,
    cwd: Option<&'a std::path::Path>,
    checked: Option<&std::path::Path>,
) -> Option<&'a std::path::Path> {
    cwd.filter(|&c| !worktree && !agent && checked != Some(c))
}

/// Una ruta en una forma que sirva para compararla (git usa `/`, Windows `\`, y no distingue mayúsculas).
fn same_path_key(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// El nombre de la carpeta (o la ruta entera si no tiene).
fn folder_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    )
}

impl Orion {
    /// Prepara la app cuando egui ya creó la ventana: la fuente y la primera pestaña.
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        grid: (u16, u16),
    ) -> Result<Self, Error> {
        crate::renderer::note_adapter(cc);
        apply_font(&cc.egui_ctx, &settings.font.family);
        menu::apply_theme(&cc.egui_ctx, &settings);
        // Sobre la escala de Windows, que eframe ya respeta (pixels_per_point = sistema × zoom).
        cc.egui_ctx.set_zoom_factor(settings.ui_scale);
        // Real OS windows for child viewports (settings), not popups trapped in the terminal.
        cc.egui_ctx.set_embed_viewports(false);
        let mut orion = Self {
            // Start on the empty home; the first terminal opens from a shortcut or the sidebar.
            tabs: Vec::new(),
            active: 0,
            settings,
            menu: SettingsMenu::default(),
            palette: Palette::default(),
            orchestrate: orchestrate::Orchestrate::restore(),
            project: Default::default(),
            review: None,
            saved: project::load_saved(),
            focused: true,
            sidebar_hidden: false,
            toast: None,
            window_title: String::new(),
            config_modified: config_modified(),
            config_checked: Instant::now(),
            perf: None,
            window: crate::backdrop::Window::of(cc),
            window_size: None,
            gdi_recheck: None,
            updater: Updater::new(),
            update_asked: false,
            logo: cc.egui_ctx.load_texture(
                "logo",
                egui::ColorImage::from_rgba_unmultiplied([64, 64], &crate::icon::render(64)),
                egui::TextureOptions::LINEAR,
            ),
            default_grid: grid,
            #[cfg(target_os = "macos")]
            mac_menu: macmenu::MacMenu::install(&cc.egui_ctx),
        };
        orion.apply_window_look();
        if orion.orchestrate.paused() {
            orion.show_toast("Quedó un plan sin terminar: ábrelo en Orquestar tarea con agentes");
        }
        // Al abrir, revisar si hay una versión nueva. En debug (`cargo run`) no: sería la del .exe de desarrollo.
        if orion.settings.check_updates && !cfg!(debug_assertions) {
            orion.updater.check(&cc.egui_ctx);
        }
        Ok(orion)
    }

    /// Los índices de `tabs` en el orden en que los muestra la barra lateral (y en que se navega).
    fn tab_order(&self) -> Vec<usize> {
        let repos: Vec<Option<&PathBuf>> = self.tabs.iter().map(|t| t.repo.as_ref()).collect();
        grouped_order(&repos)
    }

    /// La pestaña `by` lugares después de la activa, en el orden de la barra (con vuelta).
    fn tab_step(&self, by: isize) -> usize {
        let order = self.tab_order();
        if order.is_empty() {
            return 0;
        }
        let at = order.iter().position(|&i| i == self.active).unwrap_or(0) as isize;
        order[(at + by).rem_euclid(order.len() as isize) as usize]
    }

    /// El tema que sigue al actual (botón de la barra y menú de Mac).
    fn next_theme(&self) -> usize {
        let current = crate::colors::find_theme(&self.settings.theme).unwrap_or(0);
        (current + 1) % crate::colors::THEMES.len()
    }

    /// Mac: lo elegido en la barra de menú del sistema. El `bool` es Configuración, que abre o
    /// cierra la ventana como Cmd+, (macOS se queda con esa tecla para el menú).
    #[cfg(target_os = "macos")]
    fn menu_bar(&self, ctx: &egui::Context) -> (Option<Action>, bool) {
        use macmenu::Command;
        let Some(command) = self.mac_menu.next() else {
            return (None, false);
        };
        ctx.request_repaint(); // por si hay otro comando en la cola
        let action = match command {
            Command::Settings => return (None, true),
            Command::Quit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return (None, false);
            }
            Command::CheckUpdates => Action::CheckUpdates,
            Command::NewTab => Action::NewTab(None),
            Command::NewProject => Action::NewProject,
            Command::SplitRight => Action::Split(true),
            Command::SplitDown => Action::Split(false),
            Command::ClosePane => Action::ClosePane,
            Command::Palette => Action::Palette,
            Command::ToggleSidebar => Action::ToggleSidebar,
            Command::NextTheme => Action::Theme(self.next_theme()),
            Command::FontBigger => Action::FontSize(1.0),
            Command::FontSmaller => Action::FontSize(-1.0),
        };
        (Some(action), false)
    }

    /// Fuera de Mac no hay barra de menú del sistema: todo está en la ventana.
    #[cfg(not(target_os = "macos"))]
    fn menu_bar(&self, _ctx: &egui::Context) -> (Option<Action>, bool) {
        (None, false)
    }

    /// El panel con foco de la pestaña activa; None en el inicio o en una pestaña Grafo.
    fn session(&mut self) -> Option<&mut Session> {
        self.tabs.get_mut(self.active)?.focused_mut()
    }

    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// Abre una pestaña nueva del mismo tamaño que la actual, y la deja activa.
    /// `kind`: qué shell (índice en `pty::SHELLS`); None = la de config.toml.
    fn new_tab(&mut self, ctx: &egui::Context, kind: Option<usize>) {
        let grid = self
            .tabs
            .get(self.active)
            .and_then(|t| t.focused())
            .map(|s| s.grid)
            .unwrap_or(self.default_grid);
        let kind = kind.unwrap_or_else(|| default_shell(&self.settings));
        match Session::new(ctx, grid, kind) {
            Ok(session) => {
                self.tabs.push(Tab::new(session));
                self.active = self.tabs.len() - 1;
            }
            Err(e) => self.show_toast(format!("No pude abrir otra pestaña: {e}")),
        }
    }

    /// Pestaña nueva que lanza un agente: en la carpeta actual o, si el perfil lo pide, en un worktree nuevo.
    fn open_agent(&mut self, ctx: &egui::Context, index: usize) {
        let Some(agent) = self.settings.agents.get(index).cloned() else {
            return;
        };
        let cwd = self
            .tabs
            .get(self.active)
            .and_then(|t| t.focused()?.cwd.clone());
        self.launch_agent(ctx, cwd, &agent.name, agent.worktree, |_| {
            agent.command.clone()
        });
    }

    /// Pestaña nueva con un agente de `agents::PROVIDERS`, en la carpeta actual.
    fn open_provider(&mut self, ctx: &egui::Context, index: usize) {
        let Some(provider) = crate::agents::PROVIDERS.get(index) else {
            return;
        };
        let cwd = self
            .tabs
            .get(self.active)
            .and_then(|t| t.focused()?.cwd.clone());
        self.launch_agent(ctx, cwd, provider.name, false, |_| {
            provider.command.to_string()
        });
    }

    /// Pestaña nueva que corre `command` (armado para su shell) en `cwd` o, con `worktree`, en un
    /// worktree nuevo de ese repo. Devuelve false si no se pudo (y ya avisó por qué).
    fn launch_agent(
        &mut self,
        ctx: &egui::Context,
        cwd: Option<PathBuf>,
        name: &str,
        worktree: bool,
        command: impl Fn(&crate::pty::ShellKind) -> String,
    ) -> bool {
        let repo = cwd
            .as_deref()
            .and_then(|d| crate::agents::repo_root(d).ok());
        let mut dir = cwd;
        if worktree {
            let Some(cwd) = dir else {
                let tip = "abre el agente desde una pestaña de PowerShell dentro del repo";
                self.show_toast(format!("No sé en qué repo estás: {tip}"));
                return false;
            };
            match crate::agents::create_worktree(&cwd, name, None) {
                Ok(path) => dir = Some(path),
                Err(e) => {
                    self.show_toast(format!("No pude crear el worktree: {e}"));
                    return false;
                }
            }
        }
        let tabs = self.tabs.len();
        self.new_tab(ctx, None);
        if self.tabs.len() == tabs {
            return false;
        }
        let tab = &mut self.tabs[self.active];
        tab.repo = repo;
        let Some(session) = tab.focused_mut() else {
            return false; // `new_tab` siempre abre una terminal
        };
        let shell = &crate::pty::SHELLS[session.kind];
        let command = command(shell);
        let input = crate::agents::launch_input(shell, dir.as_deref(), &command);
        session.write(input.as_bytes());
        // Una terminal sola (sin comando) no es un agente. Con carpeta, `launch_input` escribe
        // antes un `cd`: el agente es el segundo comando.
        let agent = !command.is_empty();
        if agent {
            session.agent = Some(AgentWatch::new(usize::from(dir.is_some())));
        }
        tab.agent = agent.then(|| name.to_string());
        tab.agent_command = agent.then_some(command);
        tab.worktree = if worktree { dir.clone() } else { None };
        tab.agent_run = agent.then_some(tab.focus);
        true
    }

    /// Si ya hay una pestaña abierta en ese repo.
    fn repo_busy(&self, repo: &std::path::Path) -> bool {
        self.tabs.iter().any(|t| t.repo.as_deref() == Some(repo))
    }

    /// Los agentes detectados, sin los que el usuario ya definió como `[[agent]]` (mismo comando).
    fn providers(&self) -> Vec<usize> {
        let defined = |cmd: &str| self.settings.agents.iter().any(|a| a.command == cmd);
        crate::agents::installed()
            .iter()
            .copied()
            .filter(|&i| !defined(crate::agents::PROVIDERS[i].command))
            .collect()
    }

    /// Los agentes detectados pero con la instalación rota, y la ruta que falta (solo para el menú).
    fn broken_providers(&self) -> Vec<(usize, PathBuf)> {
        let defined = |cmd: &str| self.settings.agents.iter().any(|a| a.command == cmd);
        let mut broken = crate::agents::broken();
        broken.retain(|(i, _)| !defined(crate::agents::PROVIDERS[*i].command));
        broken
    }

    /// Los agentes que se pueden elegir para un proyecto, en el orden del menú.
    fn agent_choices(&self) -> Vec<AgentChoice> {
        let mut choices: Vec<AgentChoice> = self
            .providers()
            .into_iter()
            .map(AgentChoice::Provider)
            .collect();
        choices.extend((0..self.settings.agents.len()).map(AgentChoice::Profile));
        choices
    }

    /// El logo del agente de un proyecto guardado, que solo recuerda su nombre. None si ese
    /// agente ya no está ni en `PROVIDERS` ni en config.toml.
    fn saved_logo(&self, agent: &str) -> Option<Logo> {
        let profile = self.settings.agents.iter().find(|a| a.name == agent);
        profile
            .map(|a| crate::agents::logo_for(&a.command))
            .or_else(|| {
                let mut providers = crate::agents::PROVIDERS.iter();
                providers.find(|p| p.name == agent).map(|p| p.logo)
            })
    }

    /// Abre la ventana de "Nuevo proyecto", empezando en el repo de la pestaña actual si lo hay.
    fn open_project(&mut self) {
        let choices = self.agent_choices();
        self.project.open = true;
        self.project.error = None;
        // La carpeta de la vez anterior, o el repo de la pestaña actual; se vuelve a mirar si es repo.
        let cwd = self.session().and_then(|s| s.cwd.clone());
        let dir = self
            .project
            .dir
            .clone()
            .or(cwd.filter(|c| crate::agents::repo_root(c).is_ok()));
        if let Some(dir) = dir {
            self.project.set_dir(dir);
        }
        // Marcado de entrada: el agente que usaste por última vez si sigue disponible; si no, el primero.
        let last = self.saved.first().map(|s| s.agent.as_str());
        let name_of = |c: &AgentChoice| c.name(&self.settings).to_string();
        let pick = choices
            .iter()
            .find(|c| Some(c.name(&self.settings)) == last)
            .or(choices.first());
        self.project.picked = pick.map(name_of).into_iter().collect();
        self.refresh_worktree();
    }

    /// Abre una pestaña de proyecto: el agente `choice` en `dir`, o en un worktree nuevo.
    fn launch_project(
        &mut self,
        ctx: &egui::Context,
        dir: PathBuf,
        choice: AgentChoice,
        worktree: bool,
    ) {
        let name = choice.name(&self.settings).to_string();
        let command = choice.command(&self.settings);
        // La ventana se cierra solo si salió bien; si no, sigue abierta para corregir.
        if self.launch_agent(ctx, Some(dir.clone()), &name, worktree, move |_| {
            command.clone()
        }) {
            self.project.open = false;
            self.project.error = None;
            // Una carpeta que no es repo hace de proyecto igual: la pestaña se agrupa bajo ella.
            let tab = &mut self.tabs[self.active];
            let dir = tab.repo.get_or_insert(dir).clone();
            project::Saved::upsert(
                &mut self.saved,
                project::Saved {
                    dir,
                    agent: name,
                    worktree,
                },
            );
            project::save_saved(&self.saved);
        } else {
            self.project.error = Some("No se pudo abrir el proyecto (ver el aviso).".into());
        }
    }

    /// Reabre un proyecto guardado: su repo con el mismo agente y worktree.
    fn open_saved(&mut self, ctx: &egui::Context, index: usize) {
        let Some(saved) = self.saved.get(index).cloned() else {
            return;
        };
        let choice = self
            .agent_choices()
            .into_iter()
            .find(|c| c.name(&self.settings) == saved.agent);
        match choice {
            _ if !saved.dir.is_dir() => self.show_toast("La carpeta del proyecto ya no existe"),
            None => self.show_toast(format!("El agente {} ya no está disponible", saved.agent)),
            Some(choice) => self.launch_project(ctx, saved.dir, choice, saved.worktree),
        }
    }

    /// Borra los worktrees `orion/…` de los repos conocidos que ninguna pestaña usa. Los que tienen
    /// cambios sin guardar se dejan; la rama solo se borra si ya está fusionada.
    /// ponytail: corre en el hilo de la ventana; con muchos worktrees convendría un hilo aparte.
    fn clean_worktrees(&mut self) {
        let mut repos: Vec<PathBuf> = self.saved.iter().map(|s| s.dir.clone()).collect();
        repos.extend(self.tabs.iter().filter_map(|t| t.repo.clone()));
        repos.sort();
        repos.dedup();
        let in_use: Vec<String> = self
            .tabs
            .iter()
            .filter_map(|t| t.worktree.as_deref())
            .map(same_path_key)
            .collect();
        let (mut removed, mut kept) = (0, 0);
        for repo in &repos {
            for (path, branch) in crate::agents::orion_worktrees(repo) {
                if in_use.contains(&same_path_key(&path)) {
                    continue;
                }
                match crate::agents::remove_worktree(repo, &path, &branch) {
                    Ok(_) => removed += 1,
                    Err(_) => kept += 1,
                }
            }
        }
        let kept = if kept > 0 {
            format!(" ({kept} con cambios sin guardar, se dejaron)")
        } else {
            String::new()
        };
        self.show_toast(format!("Worktrees limpiados: {removed}{kept}"));
    }

    /// Cierra una pestaña (y las shells de todos sus paneles). Si era la última, queda el inicio vacío.
    fn close_tab(&mut self, ctx: &egui::Context, index: usize) {
        let mut tab = self.tabs.remove(index);
        for session in tab.panes_mut().into_iter().flat_map(Split::leaves_mut) {
            let _ = session.shell.child.kill();
        }
        // Un worktree sin trabajo (nada sin commitear ni commits propios) se borra sin preguntar.
        // ponytail: git en el hilo de la ventana (tarda poco).
        if let (Some(path), Some(repo)) = (&tab.worktree, &tab.repo) {
            let state = crate::agents::current_branch(path)
                .and_then(|b| crate::agents::branch_summary(repo, path, &b).map(|s| (b, s)));
            match state {
                Ok((branch, s)) if !s.dirty && s.commits.is_empty() => {
                    if let Err(e) = crate::agents::remove_worktree(repo, path, &branch) {
                        self.show_toast(format!("No pude borrar el worktree vacío: {e}"));
                    }
                }
                _ => self.show_toast(
                    "El worktree tiene trabajo: clic derecho → Ver cambios / Fusionar antes de cerrar",
                ),
            }
        }
        if self.tabs.is_empty() {
            self.active = 0;
            if self.window_title != "Orion" {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title("Orion".into()));
                self.window_title = "Orion".into();
            }
            return;
        }
        self.active = active_after_close(self.active, index, self.tabs.len());
    }

    /// Enfoca la pestaña Grafo del repo de la pestaña `i`, o abre una (queda activa: así carga).
    fn open_graph(&mut self, i: usize) {
        let Some(repo) = self.tabs.get(i).and_then(|t| t.repo.clone()) else {
            return;
        };
        let graph =
            |t: &Tab| matches!(t.content, Content::Graph(_)) && t.repo == Some(repo.clone());
        self.active = match self.tabs.iter().position(graph) {
            Some(open) => open,
            None => {
                self.tabs.push(Tab::graph(repo));
                self.tabs.len() - 1
            }
        };
    }

    /// Abre "Ver cambios" para la pestaña `i` (que tiene worktree).
    fn open_review(&mut self, i: usize) {
        let Some(tab) = self.tabs.get(i) else { return };
        let (Some(repo), Some(worktree)) = (&tab.repo, &tab.worktree) else {
            return;
        };
        match review::Review::load(repo, worktree.clone()) {
            Ok(review) => self.review = Some(review),
            Err(e) => self.show_toast(format!("No pude leer los cambios: {e}")),
        }
    }

    /// Fusiona la rama de la pestaña `i` en la rama del repo principal y avisa el resultado.
    /// ponytail: git en el hilo de la ventana (tarda poco).
    fn merge_tab(&mut self, i: usize) {
        let Some(tab) = self.tabs.get(i) else { return };
        let (Some(repo), Some(worktree)) = (&tab.repo, &tab.worktree) else {
            return;
        };
        let result = crate::agents::current_branch(worktree).and_then(|branch| {
            let into = crate::agents::current_branch(repo)?;
            crate::agents::merge_branch(repo, &branch)
                .map(|_| format!("{branch} fusionada en {into}"))
        });
        self.review = None;
        self.show_toast(result.unwrap_or_else(|e| format!("No se fusionó: {e}")));
    }

    /// Borra el worktree y la rama de la pestaña `i` y la cierra. La confirmación ya la dio la ventana.
    /// ponytail: git en el hilo de la ventana (tarda poco).
    fn discard_tab(&mut self, ctx: &egui::Context, i: usize) {
        let Some(tab) = self.tabs.get_mut(i) else {
            return;
        };
        let (Some(repo), Some(worktree)) = (tab.repo.clone(), tab.worktree.take()) else {
            return;
        };
        let branch = crate::agents::current_branch(&worktree);
        // Primero se cierran las shells (en Windows bloquean la carpeta) y la pestaña, sin el aviso de worktree.
        self.close_tab(ctx, i);
        self.review = None;
        let result =
            branch.and_then(|b| crate::agents::discard_worktree(&repo, &worktree, &b).map(|_| b));
        self.show_toast(match result {
            Ok(branch) => format!("Rama {branch} descartada"),
            Err(e) => format!("No pude descartar el worktree: {e}"),
        });
    }

    /// Divide el panel con foco con una shell nueva, que queda con el foco.
    fn split(&mut self, ctx: &egui::Context, right: bool) {
        // El panel nuevo abre la misma shell, y se ajusta a su tamaño al dibujarse.
        let Some(&mut Session { grid, kind, .. }) = self.session() else {
            return; // una pestaña Grafo no se divide
        };
        match Session::new(ctx, grid, kind) {
            Ok(session) => {
                let id = session.id;
                let tab = &mut self.tabs[self.active];
                let focus = tab.focus;
                if let Some(panes) = tab.panes_mut()
                    && panes.split(focus, right, session).is_ok()
                {
                    tab.focus = id;
                }
            }
            Err(e) => self.show_toast(format!("No pude abrir otro panel: {e}")),
        }
    }

    /// Cierra el panel `id` de la pestaña `index` (y su shell). Si era el único (o la pestaña es
    /// un Grafo, que no tiene paneles), se cierra la pestaña.
    fn close_pane(&mut self, ctx: &egui::Context, index: usize, id: u64) {
        let tab = &mut self.tabs[index];
        let (focus, rects) = (tab.focus, &tab.rects);
        let Some(panes) = (match &mut tab.content {
            Content::Terminal(panes) if panes.leaves().len() > 1 => Some(panes),
            _ => None,
        }) else {
            self.close_tab(ctx, index);
            return;
        };
        // El foco pasa al vecino más cercano: el de la izquierda, arriba, derecha o abajo.
        let dirs = [(-1.0, 0.0), (0.0, -1.0), (1.0, 0.0), (0.0, 1.0)];
        let next = dirs
            .into_iter()
            .find_map(|(x, y)| panes::neighbor(rects, id, egui::vec2(x, y)));
        if let Some(mut session) = panes.remove(id) {
            let _ = session.shell.child.kill();
        }
        if panes.get(focus).is_none() {
            let first = panes.leaves()[0].id;
            tab.focus = next.filter(|&n| panes.get(n).is_some()).unwrap_or(first);
        }
    }

    /// Atajos de pestañas y paneles. Se "consumen": así no llegan a la shell.
    /// `COMMAND` es Ctrl en Windows y Cmd en Mac.
    fn shortcuts(&self, ui: &mut egui::Ui) -> Option<Action> {
        use egui::{Key, Modifiers};
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let _count = self.tabs.len();
        ui.input_mut(|i| {
            // En Mac estos cinco son del menú del sistema (ver `macmenu`): un solo camino.
            if !MAC {
                if i.consume_key(cmd_shift, Key::P) {
                    return Some(Action::Palette);
                }
                if i.consume_key(cmd_shift, Key::T) {
                    return Some(Action::NewTab(None));
                }
                if i.consume_key(cmd_shift, Key::O) {
                    return Some(Action::NewProject);
                }
                // Splits: D = a la derecha, E = hacia abajo. W cierra el panel.
                if i.consume_key(cmd_shift, Key::D) {
                    return Some(Action::Split(true));
                }
                if i.consume_key(cmd_shift, Key::E) {
                    return Some(Action::Split(false));
                }
                if i.consume_key(cmd_shift, Key::W) {
                    return Some(Action::ClosePane);
                }
                // Ctrl+B: mostrar u ocultar la barra lateral (en Mac es Cmd+B, del menú del sistema).
                if i.consume_key(Modifiers::COMMAND, Key::B) {
                    return Some(Action::ToggleSidebar);
                }
            }
            // Ctrl+Alt+flechas (Cmd+Option en Mac): pasar al panel de al lado.
            let arrows = [
                (Key::ArrowLeft, egui::vec2(-1.0, 0.0)),
                (Key::ArrowRight, egui::vec2(1.0, 0.0)),
                (Key::ArrowUp, egui::vec2(0.0, -1.0)),
                (Key::ArrowDown, egui::vec2(0.0, 1.0)),
            ];
            for (key, dir) in arrows {
                if i.consume_key(Modifiers::COMMAND | Modifiers::ALT, key) {
                    return Some(Action::Focus(dir));
                }
            }
            // Ctrl+Tab también en Mac: Cmd+Tab es del sistema (cambiar de app).
            // Primero Ctrl+Shift+Tab: si no, Ctrl+Tab también lo atraparía.
            if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab) {
                return Some(Action::SelectTab(self.tab_step(-1)));
            }
            if i.consume_key(Modifiers::CTRL, Key::Tab) {
                return Some(Action::SelectTab(self.tab_step(1)));
            }
            // Alt+1 … Alt+9 (Cmd+1 … Cmd+9 en Mac): ir a esa pestaña.
            const NUMBERS: [Key; 9] = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            let tab_mod = if MAC {
                Modifiers::COMMAND
            } else {
                Modifiers::ALT
            };
            let n = NUMBERS
                .iter()
                .position(|&key| i.consume_key(tab_mod, key))?;
            // El número es la posición en la barra, no el orden de apertura.
            self.tab_order().get(n).map(|&tab| Action::SelectTab(tab))
        })
    }

    fn apply_action(&mut self, ctx: &egui::Context, action: Action) {
        // Home (no tabs): only actions that open something or tweak the chrome.
        if self.tabs.is_empty()
            && !matches!(
                action,
                Action::NewTab(_)
                    | Action::Agent(_)
                    | Action::Provider(_)
                    | Action::Orchestrate
                    | Action::NewProject
                    | Action::OpenSaved(_)
                    | Action::ForgetSaved(_)
                    | Action::Palette
                    | Action::ToggleSidebar
                    | Action::OpenSettings
                    | Action::AddAgent
                    | Action::FontSize(_)
                    | Action::Theme(_)
                    | Action::TogglePerf
                    | Action::Backdrop(_)
                    | Action::CheckUpdates
                    | Action::InstallUpdate(_)
                    | Action::RestartForUpdate
                    | Action::CleanWorktrees
            )
        {
            return;
        }
        match action {
            Action::NewTab(kind) => self.new_tab(ctx, kind),
            Action::Agent(i) => self.open_agent(ctx, i),
            Action::Provider(i) => self.open_provider(ctx, i),
            Action::Orchestrate => self.open_orchestrate(),
            Action::NewProject => self.open_project(),
            Action::InProject(tab, choice, worktree) => {
                if let Some(repo) = self.tabs.get(tab).and_then(|t| t.repo.clone()) {
                    self.launch_project(ctx, repo, choice, worktree);
                }
            }
            Action::TerminalIn(tab) => {
                if let Some(repo) = self.tabs.get(tab).and_then(|t| t.repo.clone()) {
                    self.launch_agent(ctx, Some(repo), "terminal", false, |_| String::new());
                }
            }
            Action::CleanWorktrees => self.clean_worktrees(),
            Action::OpenSaved(i) => self.open_saved(ctx, i),
            Action::Review(i) => self.open_review(i),
            Action::Graph(i) => self.open_graph(i),
            Action::Merge(i) => self.merge_tab(i),
            Action::Discard(i) => self.discard_tab(ctx, i),
            Action::ForgetSaved(i) => {
                if i < self.saved.len() {
                    self.saved.remove(i);
                    project::save_saved(&self.saved);
                }
            }
            Action::CloseTab(i) => self.close_tab(ctx, i),
            Action::SelectTab(i) => self.active = i,
            Action::Split(right) => self.split(ctx, right),
            Action::ClosePane => {
                let id = self.tabs[self.active].focus;
                self.close_pane(ctx, self.active, id);
            }
            Action::Focus(dir) => {
                let tab = &mut self.tabs[self.active];
                if let Some(id) = panes::neighbor(&tab.rects, tab.focus, dir) {
                    tab.focus = id;
                }
            }
            Action::Palette => self.palette.toggle(),
            Action::ToggleSidebar => self.sidebar_hidden = !self.sidebar_hidden,
            Action::OpenSettings => self.open_menu(),
            Action::AddAgent => self.open_agents_settings(ctx),
            Action::FontSize(delta) => {
                let mut new = self.settings.clone();
                new.font.size = (new.font.size + delta).clamp(8.0, 48.0);
                self.change_settings(ctx, new);
            }
            Action::TogglePerf => {
                self.perf = if self.perf.is_some() {
                    None
                } else {
                    Some(Perf::new())
                };
            }
            Action::CheckUpdates => {
                self.update_asked = true;
                self.updater.check(ctx);
            }
            Action::InstallUpdate(version) => self.updater.install(ctx, version),
            Action::RestartForUpdate => match update::restart() {
                Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => self.show_toast(format!("No pude reiniciar: {e}")),
            },
            Action::Backdrop(i) => {
                let mut new = self.settings.clone();
                new.backdrop = crate::backdrop::BACKDROPS[i].0.into();
                self.change_settings(ctx, new);
            }
            Action::Theme(i) => {
                let mut new = self.settings.clone();
                new.theme = crate::colors::THEMES[i].name.into();
                self.change_settings(ctx, new);
            }
        }
    }

    /// Si guardaste config.toml desde otro editor, lo vuelve a leer y aplica los cambios.
    fn watch_config(&mut self, ctx: &egui::Context) {
        // egui duerme si nada pasa: le pedimos despertar para volver a mirar.
        ctx.request_repaint_after(CONFIG_POLL);
        if self.config_checked.elapsed() < CONFIG_POLL {
            return;
        }
        self.config_checked = Instant::now();
        let modified = config_modified();
        if modified == self.config_modified {
            return;
        }
        self.config_modified = modified;
        match reload_settings() {
            // Si es lo mismo que ya tenemos (ej. lo acaba de guardar el menú), no hay nada que avisar.
            Ok(new) if new == self.settings => {}
            Ok(new) => {
                self.apply_settings(ctx, new);
                self.show_toast("Configuración recargada");
            }
            // Mientras tenga errores se sigue usando la configuración anterior.
            Err(e) => self.show_toast(format!("Error en config.toml, {e}")),
        }
    }

    /// Avisos de este frame: los del panel que estás mirando se descartan (ya lo ves);
    /// los demás marcan su pestaña y panel. Si Orion no está en primer plano, parpadea en la barra de tareas.
    fn collect_alerts(&mut self, ctx: &egui::Context) {
        let (active, focused) = (self.active, self.focused);
        let mut new_alert = false;
        let mut failed = None;
        for (t, tab) in self.tabs.iter_mut().enumerate() {
            let focus = tab.focus;
            // Un agente que termina con error avisa una sola vez, como cualquier otra alerta.
            if let (Some(AgentStatus::Done(Some(code))), false) =
                (tab.agent_status(), tab.agent_notified)
                && code != 0
                && let Some(name) = tab.agent.clone()
                && let Some(pane) = tab.agent_run
                && let Some(session) = tab.panes_mut().and_then(|p| p.get_mut(pane))
            {
                session.alert = Some(format!(
                    "El agente {name} terminó con error (código {code})"
                ));
                failed = session.alert.clone();
                tab.agent_notified = true;
            }
            for session in tab.panes_mut().into_iter().flat_map(Split::leaves_mut) {
                let Some(alert) = session.alert.take() else {
                    continue;
                };
                if !(focused && t == active && session.id == focus) {
                    session.attention = Some(alert);
                    new_alert = true;
                }
            }
        }
        if let Some(text) = failed {
            self.show_toast(text);
        }
        // Lo que estás mirando ya no pide atención.
        if focused && let Some(session) = self.session() {
            session.attention = None;
        }
        if new_alert && !focused {
            let blink = egui::UserAttentionType::Informational;
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(blink));
        }
    }

    /// Dibuja los paneles de la pestaña activa en `area`.
    /// Devuelve true si algún panel pidió abrir la configuración.
    fn draw_panes(&mut self, ui: &mut egui::Ui, area: egui::Rect) -> bool {
        let ctx = ui.ctx().clone();
        // Con el menú o la paleta abiertos, la rueda y el mouse son para ellos.
        let menu_open =
            self.menu.open || self.palette.open || self.orchestrate.open || self.project.open;
        // The panes float inside the frame: a gap on the right, below and between them. The
        // title bar and the sidebar already frame the other two sides.
        let left = if self.sidebar_hidden { PANE_GAP } else { 0.0 };
        let inner = egui::Rect::from_min_max(
            area.min + egui::vec2(left, 0.0),
            area.max - egui::Vec2::splat(PANE_GAP),
        );
        // Con una ventana de Orion encima, el teclado no es del Grafo.
        let keys = !(self.menu.open
            || self.palette.open
            || self.orchestrate.open
            || self.project.open
            || self.review.is_some());
        let tab = &mut self.tabs[self.active];
        tab.rects.clear();
        let panes = match &mut tab.content {
            Content::Terminal(panes) => panes,
            Content::Graph(graph) => {
                // Un solo "panel", con el mismo marco y fondo que una terminal.
                let frame = surface(theme().sidebar());
                let hole = [inner.shrink(1.0)];
                chrome::paint_around(ui.painter(), area, hole, PANE_RADIUS - 1.0, frame);
                let builder = egui::UiBuilder::new().max_rect(inner).id_salt("grafo");
                let mut pane_ui = ui.new_child(builder);
                egui::Frame::central_panel(pane_ui.style())
                    .fill(surface(theme().bg))
                    .corner_radius(PANE_RADIUS)
                    .show(&mut pane_ui, |ui| {
                        graph.keys = keys;
                        graph.ui(ui);
                    });
                let stroke = egui::Stroke::new(1.0, theme().line());
                ui.painter()
                    .rect_stroke(inner, PANE_RADIUS, stroke, egui::StrokeKind::Inside);
                return false;
            }
        };
        panes.layout(inner, PANE_GAP, &mut tab.rects);
        // The frame goes one pixel under each pane, so no seam shows between them.
        let holes = tab.rects.iter().map(|(_, rect)| rect.shrink(1.0));
        let frame = surface(theme().sidebar());
        chrome::paint_around(ui.painter(), area, holes, PANE_RADIUS - 1.0, frame);

        // Clic en un panel: pasa a tener el foco (si el clic no fue en un menú o ventana de encima).
        let press = ui.input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()));
        if let Some(pos) = press.flatten()
            && ctx.layer_id_at(pos) == Some(ui.layer_id())
            && let Some(&(id, _)) = tab.rects.iter().find(|(_, r)| r.contains(pos))
        {
            tab.focus = id;
        }

        let several = tab.rects.len() > 1;
        let (mut open_menu, mut split, mut toast) = (false, None, None);
        for &(id, rect) in &tab.rects {
            let Some(session) = panes.get_mut(id) else {
                continue;
            };
            // Cada panel dibuja en su pedazo; id_salt separa los ids de egui de un panel y otro.
            let mut pane_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(id));
            let active = id == tab.focus;
            open_menu |= session.draw(&mut pane_ui, &self.settings.font, menu_open, active);
            if several && !active {
                // Los paneles sin foco, un poco más oscuros: se ve dónde vas a escribir.
                let dim = egui::Color32::from_black_alpha(if theme().is_light() { 16 } else { 60 });
                ui.painter().rect_filled(rect, PANE_RADIUS, dim);
            }
            // A thin border. It takes the accent on the pane that asks for attention and,
            // with several panes, a softer accent on the one that has the focus.
            let stroke = if session.attention.is_some() {
                egui::Stroke::new(2.0, theme().accent())
            } else if several && active {
                egui::Stroke::new(1.0, theme().accent().gamma_multiply(0.6))
            } else {
                egui::Stroke::new(1.0, theme().line())
            };
            ui.painter()
                .rect_stroke(rect, PANE_RADIUS, stroke, egui::StrokeKind::Inside);
            if let Some(right) = session.split_request.take() {
                tab.focus = id;
                split = Some(right);
            }
            toast = session.toast.take().or(toast);
        }
        if let Some(right) = split {
            self.split(&ctx, right);
        }
        if let Some(text) = toast {
            self.show_toast(text);
        }
        open_menu
    }
}

/// La shell de config.toml, o la primera instalada si esa no está.
fn default_shell(settings: &Settings) -> usize {
    usable_shell(find_shell(&settings.shell).unwrap_or(0))
}

/// Qué pestaña queda activa al cerrar la `closed`, si quedan `left`.
/// La activa sigue siendo la misma pestaña: si cerraste una de su izquierda, su índice baja uno.
/// Si cerraste la activa, queda la de su derecha (o la de su izquierda si era la última).
fn active_after_close(active: usize, closed: usize, left: usize) -> usize {
    let active = if closed < active { active - 1 } else { active };
    active.min(left.saturating_sub(1))
}

impl eframe::App for Orion {
    // Lo que no pintamos queda transparente: así se ve Mica o el acrílico.
    // Sin esos fondos, la barra lateral y los paneles cubren toda la ventana con colores opacos.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    // logic corre antes de cada ui y también con la ventana oculta: así ningún canal se acumula.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Al cambiar de tamaño, Windows deja vieja la capa GDI de la ventana (ver `clear_gdi_layer`).
        let size = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size());
        // Se limpia al cambiar y otra vez un rato después: al abrir, eframe muestra la ventana
        // después del primer frame y Windows vuelve a llenar la capa.
        let changed = size != self.window_size;
        let recheck = self.gdi_recheck.is_some_and(|t| Instant::now() >= t);
        if changed || recheck {
            self.window_size = size;
            self.gdi_recheck = changed.then(|| Instant::now() + GDI_RECHECK);
            if let Some(window) = self.window
                && crate::renderer::active() == crate::renderer::Gpu::DirectX12
            {
                crate::backdrop::clear_gdi_layer(window);
            }
        }
        if self.gdi_recheck.is_some() {
            ctx.request_repaint_after(GDI_RECHECK);
        }
        // Ctrl+= / Ctrl+- / Ctrl+0 son el zoom de egui: lo guardamos como ui_scale.
        let zoom = ctx.zoom_factor();
        if (zoom - self.settings.ui_scale).abs() > 0.001 {
            let mut new = self.settings.clone();
            new.ui_scale = ((zoom * 100.0).round() / 100.0).clamp(UI_SCALE.0, UI_SCALE.1);
            self.change_settings(ctx, new);
        }
        if self.tabs.is_empty() {
            self.watch_config(ctx);
            match self.updater.state() {
                UpdateState::UpToDate if self.update_asked => {
                    self.show_toast(format!("Orion está al día ({})", update::CURRENT));
                    self.update_asked = false;
                }
                UpdateState::CheckFailed(e) if self.update_asked => {
                    self.show_toast(format!("Actualización: {e}"));
                    self.update_asked = false;
                    self.updater.dismiss();
                }
                UpdateState::Failed(e) => {
                    self.show_toast(format!("Actualización: {e}"));
                    self.updater.dismiss();
                }
                _ => {}
            }
            return;
        }
        // Todos los paneles de todas las pestañas: los de atrás siguen recibiendo su salida.
        let mut pending = false;
        let mut copied = None;
        for tab in &mut self.tabs {
            for session in tab.panes_mut().into_iter().flat_map(Split::leaves_mut) {
                pending |= session.process_output();
                copied = session.take_copied().or(copied);
            }
        }
        // Un programa copió algo (OSC 52, ej. "copiar respuesta" en un agente): al portapapeles.
        if let Some(text) = copied {
            let lines = text.lines().count().max(1);
            ctx.copy_text(text);
            let what = if lines == 1 {
                "1 línea".into()
            } else {
                format!("{lines} líneas")
            };
            self.show_toast(format!("Copiado al portapapeles ({what})"));
        }
        if pending {
            ctx.request_repaint(); // quedó salida: seguir en el próximo frame
        }
        // Paneles cuya shell terminó (escribiste `exit`): se cierran solos.
        while let Some((t, id)) = self.tabs.iter_mut().enumerate().find_map(|(t, tab)| {
            let mut sessions = tab.panes_mut().into_iter().flat_map(Split::leaves_mut);
            sessions.find_map(|s| s.exited().then_some((t, s.id)))
        }) {
            self.close_pane(ctx, t, id);
            if self.tabs.is_empty() {
                return;
            }
        }
        // Una terminal suelta que hizo cd a un repo pasa a agruparse bajo él (y al revés).
        // git responde desde un hilo: arrancarlo en el de la ventana se nota en cada `cd`.
        for tab in &mut self.tabs {
            if let Some(rx) = &tab.repo_check {
                match rx.try_recv() {
                    Ok(repo) => {
                        // Entretanto la pestaña pudo pasar a ser de un agente: su repo es fijo.
                        if tab.worktree.is_none() && tab.agent.is_none() {
                            tab.repo = repo;
                        }
                        tab.repo_check = None;
                    }
                    Err(TryRecvError::Empty) => continue,
                    Err(TryRecvError::Disconnected) => tab.repo_check = None,
                }
            }
            let cwd = tab.focused().and_then(|s| s.cwd.as_deref());
            let worktree = tab.worktree.is_some();
            let agent = tab.agent.is_some();
            if let Some(cwd) = repo_to_check(worktree, agent, cwd, tab.repo_cwd.as_deref()) {
                let cwd = cwd.to_path_buf();
                tab.repo_cwd = Some(cwd.clone());
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(crate::agents::repo_root(&cwd).ok());
                    ctx.request_repaint();
                });
                tab.repo_check = Some(rx);
            }
        }
        // Pestañas Grafo: recoger lo que terminó, recargar la que vuelve a estar activa y vigilar el repo.
        let mut terminals = Vec::new();
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if let Content::Graph(graph) = &mut tab.content {
                graph.poll(ctx, i == self.active);
                terminals.extend(graph.take_terminal());
            }
        }
        // "Abrir una terminal aquí" en un worktree: igual que "Terminal en la carpeta del repo".
        for dir in terminals {
            self.launch_agent(ctx, Some(dir), "terminal", false, |_| String::new());
        }
        self.watch_config(ctx);
        self.collect_alerts(ctx);
        // Resultado de buscar actualizaciones a mano.
        match self.updater.state() {
            UpdateState::UpToDate if self.update_asked => {
                self.show_toast(format!("Orion está al día ({})", update::CURRENT));
                self.update_asked = false;
            }
            UpdateState::CheckFailed(e) if self.update_asked => {
                self.show_toast(format!("Actualización: {e}"));
                self.update_asked = false;
                self.updater.dismiss();
            }
            // Instalar siempre lo pediste con un clic: el error se muestra siempre.
            UpdateState::Failed(e) => {
                self.show_toast(format!("Actualización: {e}"));
                self.updater.dismiss();
            }
            _ => {}
        }
        // El título de la ventana sigue al del panel con foco.
        let title = format!("{} — Orion", self.tabs[self.active].title());
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Si la app lo pidió (ESC[?1004h, ej. vim), avisarle cuando la ventana gana o pierde el foco.
        let focused = ui.input(|i| i.focused);
        if focused != self.focused
            && let Some(session) = self.session()
            && session.term.mode().contains(TermMode::FOCUS_IN_OUT)
        {
            session.write(if focused { b"\x1b[I" } else { b"\x1b[O" });
        }
        self.focused = focused;

        // Archivos arrastrados: las fuentes se instalan; el resto se escribe como rutas, como un pegado.
        let dropped: Vec<PathBuf> = ui.input(|i| {
            let files = i.raw.dropped_files.iter();
            files.map(|f| f.path().to_path_buf()).collect()
        });
        let (fonts, others): (Vec<_>, Vec<_>) = dropped
            .into_iter()
            .partition(|p| is_font_file(&p.to_string_lossy()));
        for path in fonts {
            self.font_dropped(&ctx, &path);
        }
        if !others.is_empty()
            && let Some(session) = self.session()
        {
            let shell = &crate::pty::SHELLS[session.kind];
            let paths: Vec<String> = others
                .iter()
                .map(|p| shell.quote_path(&p.to_string_lossy()))
                .collect();
            let bracketed = session.term.mode().contains(TermMode::BRACKETED_PASTE);
            session.write(&crate::input::paste_bytes(
                &format!("{} ", paths.join(" ")),
                bracketed,
            ));
        }

        // Pestañas y paneles: barra lateral y atajos.
        let shortcut = if self.menu.open {
            None
        } else {
            self.shortcuts(ui)
        };
        let bar = self.title_bar(ui);
        let (menu_bar, menu_toggle) = self.menu_bar(&ctx);
        if let Some(action) = menu_bar.or(self.tab_sidebar(ui)).or(bar).or(shortcut) {
            self.apply_action(&ctx, action);
        }
        let empty = self.tabs.is_empty();

        // Ctrl+, (Cmd+, en Mac) abre o cierra la configuración; Escape la cierra.
        let (toggle, escape) = ui.input(|i| {
            let toggle = i.modifiers.command && i.key_pressed(egui::Key::Comma);
            (toggle, i.key_pressed(egui::Key::Escape))
        });
        let toggle = toggle || menu_toggle;
        if toggle && !self.menu.open {
            self.open_menu();
        } else if self.menu.open && (toggle || escape) {
            self.menu.open = false;
        } else if !self.menu.open
            && !self.palette.open
            && !self.orchestrate.open
            && !self.project.open
            && !toggle
            && let Some(session) = self.session()
        {
            // En una pestaña Grafo no hay shell: el teclado queda para la vista.
            session.send_input(ui);
        }
        // Con el menú, la paleta o el orquestador abiertos, el teclado es para ellos, no para la shell.

        let area = ui.available_rect_before_wrap();
        if empty {
            if let Some(action) = self.draw_home(ui, area) {
                self.apply_action(&ctx, action);
            }
        } else if self.draw_panes(ui, area) {
            self.open_menu();
        }
        if let Some(toast) = &self.toast {
            render::draw_toast(ui, area, toast, self.settings.font.size);
        }
        if let Some(perf) = &mut self.perf {
            // cpu_usage es del frame anterior (este todavía no terminó).
            perf.frame(frame.info().cpu_usage.unwrap_or(0.0));
            let text = format!("{:.1} ms/frame · {} frames/s", perf.avg_ms, perf.fps);
            let font = egui::FontId::monospace(12.0);
            let pos = area.right_top() + egui::vec2(-10.0, 6.0);
            let color = theme().ansi[3];
            ui.painter()
                .text(pos, egui::Align2::RIGHT_TOP, text, font, color);
            // Para que el contador se actualice aunque no pase nada.
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        self.settings_window(&ctx);
        if let Some(action) = self.review_window(&ctx) {
            self.apply_action(&ctx, action);
        }
        if let Some(subtasks) = self.orchestrate_window(&ctx) {
            self.launch_plan(&ctx, subtasks);
        }
        self.advance_plan(&ctx);
        if let Some((dir, choices, worktree)) = self.project_window(&ctx) {
            for choice in choices {
                self.launch_project(&ctx, dir.clone(), choice, worktree);
            }
        }
        // Al final: una acción de la paleta (ej. cerrar la última pestaña) puede cerrar la ventana.
        if let Some(action) = self.palette_window(&ctx) {
            self.apply_action(&ctx, action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_tabs_keeps_the_right_one_active() {
        // 4 pestañas, la activa es la 2.
        assert_eq!(active_after_close(2, 0, 3), 1); // cerré una de la izquierda: se corre
        assert_eq!(active_after_close(2, 3, 3), 2); // cerré una de la derecha: no cambia
        assert_eq!(active_after_close(2, 2, 3), 2); // cerré la activa: queda la de su derecha
        assert_eq!(active_after_close(3, 3, 3), 2); // cerré la activa y era la última: la de la izquierda
    }

    #[test]
    fn tabs_are_grouped_by_repo_with_loose_terminals_last() {
        let (a, b) = (PathBuf::from("/a"), PathBuf::from("/b"));
        // abiertas: terminal, a, b, a, terminal
        let repos = [None, Some(&a), Some(&b), Some(&a), None];
        assert_eq!(grouped_order(&repos), vec![1, 3, 2, 0, 4]);
        assert_eq!(grouped_order(&[None, None]), vec![0, 1]);
    }

    #[test]
    fn only_loose_terminals_follow_their_cwd_to_a_repo() {
        let (a, b) = (PathBuf::from("/a"), PathBuf::from("/b"));
        let check = |worktree, agent, cwd: Option<&PathBuf>, checked: Option<&PathBuf>| {
            repo_to_check(
                worktree,
                agent,
                cwd.map(|p| p.as_path()),
                checked.map(|p| p.as_path()),
            )
            .map(PathBuf::from)
        };
        assert_eq!(check(false, false, Some(&a), None), Some(a.clone())); // primer cwd
        assert_eq!(check(false, false, Some(&b), Some(&a)), Some(b.clone())); // hizo cd
        assert_eq!(check(false, false, Some(&a), Some(&a)), None); // mismo cwd: sin git
        assert_eq!(check(false, false, None, Some(&a)), None); // la shell no avisa cwd
        assert_eq!(check(true, false, Some(&b), Some(&a)), None); // worktree: fijo
        assert_eq!(check(false, true, Some(&b), Some(&a)), None); // agente: fijo
    }

    #[test]
    fn a_tab_names_its_agent_unless_the_title_already_does() {
        assert_eq!(tab_label(None, "benja"), "benja");
        assert_eq!(tab_label(Some("Codex"), "orion-1"), "Codex · orion-1");
        assert_eq!(tab_label(Some("OpenCode"), "OpenCode"), "OpenCode");
    }

    #[test]
    fn a_tab_gets_the_logo_of_its_agent() {
        let logo = agent_logo;
        // Pestaña que abrió Orion: por el comando, aunque corra con argumentos.
        let launched = Some("codex 'arregla el test'");
        assert_eq!(logo(None, launched, "orion"), Some(Logo::Codex));
        assert_eq!(logo(None, Some("mi-agente"), "orion"), Some(Logo::Generic));
        // Abierto a mano; y lo que corre ahora manda sobre lo que se lanzó.
        assert_eq!(logo(Some("aider"), None, "orion"), Some(Logo::Aider));
        assert_eq!(logo(Some("gemini"), launched, "x"), Some(Logo::Gemini));
        assert_eq!(logo(Some("git log"), launched, "x"), Some(Logo::Codex));
        // Sin bloques, queda el título.
        assert_eq!(logo(None, None, "✳ Claude Code"), Some(Logo::Claude));
        // Una terminal cualquiera no lleva logo, ni una carpeta que se llame parecido.
        assert_eq!(logo(Some("cargo build"), None, "orion"), None);
        assert_eq!(logo(None, None, "codex"), None);
        assert_eq!(plain_title("⠋ OpenCode"), "OpenCode");
        assert_eq!(plain_title("~/code"), "~/code");
        assert_eq!(plain_title("✳"), "✳");
    }

    #[test]
    fn profile_choice_reads_the_agents_settings() {
        let mut settings = Settings::default();
        settings.agents.push(crate::config::Agent {
            name: "Mi agente".into(),
            id: None,
            use_for: None,
            executor: None,
            command: "mi-agente".into(),
            worktree: true,
        });
        let choice = AgentChoice::Profile(0);
        assert_eq!(choice.name(&settings), "Mi agente");
        assert_eq!(choice.command(&settings), "mi-agente");
        assert!(choice.worktree(&settings, false)); // el perfil lo pide
        assert!(!AgentChoice::Provider(0).worktree(&settings, false)); // primero en la carpeta
        assert!(AgentChoice::Provider(0).worktree(&settings, true)); // el repo ya está ocupado
    }
}
