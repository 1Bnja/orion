//! La app: las pestañas, la configuración y lo que pasa en cada frame.
//!
//! Cada pestaña tiene uno o más paneles (splits) y cada panel es una `Session` (su shell, su grilla,
//! sus bloques). `session`, `panes`, `render` y `menu` son submódulos: al ser hijos de `app`,
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

#[cfg(target_os = "macos")]
mod macmenu;
mod menu;
mod orchestrate;
mod palette;
mod panes;
mod project;
mod render;
mod session;
mod titlebar;

use crate::colors::{opaque, surface, theme};
use crate::config::{Settings, config_modified, reload_settings};
use crate::fonts::{apply_font, is_font_file};
use crate::pty::{Error, SHELLS, find_shell, installed_shells, usable_shell};
use crate::update::{self, State as UpdateState, Updater};
use alacritty_terminal::term::TermMode;
use eframe::egui;
use menu::SettingsMenu;
use palette::Palette;
use panes::Split;
use project::Project;
use session::Session;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// Una pestaña: sus paneles y cuál tiene el foco.
struct Tab {
    panes: Split<Session>,
    /// El `id` del panel que recibe el teclado.
    focus: u64,
    /// Dónde se dibujó cada panel en el último frame (para mover el foco con Ctrl+Alt+flechas).
    rects: Vec<(u64, egui::Rect)>,
    /// Raíz del repo al que pertenece (para agrupar la barra lateral); None = terminal suelta.
    repo: Option<PathBuf>,
    /// El agente con que se abrió, para distinguir varias pestañas de un mismo proyecto.
    agent: Option<String>,
    /// La carpeta del worktree si la pestaña nació en uno (para no limpiarlo mientras se usa).
    worktree: Option<PathBuf>,
}

impl Tab {
    fn new(session: Session) -> Self {
        Self {
            repo: None,
            agent: None,
            worktree: None,
            focus: session.id,
            panes: Split::new(session),
            rects: Vec::new(),
        }
    }

    fn focused(&self) -> &Session {
        self.panes
            .get(self.focus)
            .expect("el foco apunta a un panel")
    }

    fn focused_mut(&mut self) -> &mut Session {
        self.panes
            .get_mut(self.focus)
            .expect("el foco apunta a un panel")
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
    /// Proyectos guardados (projects.json).
    saved: Vec<project::Saved>,
    /// Si la ventana tenía el foco en el frame anterior (para avisar cambios a la app de la pestaña).
    focused: bool,
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
    updater: Updater,
    /// Buscaste actualizaciones a mano: avisar también si está al día o si falló.
    /// (La revisión automática al abrir es silenciosa: sin internet no hay nada que molestar.)
    update_asked: bool,
    /// El ícono de Orion, para la barra de arriba.
    logo: egui::TextureHandle,
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

/// Cada cuánto se revisa si config.toml cambió.
/// ponytail: se pregunta la fecha del archivo (stdlib); el crate `notify` avisaría al instante, sin preguntar.
const CONFIG_POLL: Duration = Duration::from_secs(1);

/// Algo que se pidió sobre las pestañas o los paneles en este frame.
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
    OpenSettings,
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

/// Lo que dice la fila de una pestaña: el agente y la carpeta, salvo que el título ya nombre al agente
/// (varios agentes ponen su nombre de título de la terminal).
fn tab_label(agent: Option<&str>, title: &str) -> String {
    match agent {
        Some(a) if !title.to_lowercase().contains(&a.to_lowercase()) => format!("{a} · {title}"),
        _ => title.to_string(),
    }
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

/// Encabezado de un grupo de la barra lateral; salvo el primero, lo separa una línea fina.
fn group_header(
    ui: &mut egui::Ui,
    width: f32,
    label: &str,
    tip: Option<&std::path::Path>,
    first: bool,
) -> Option<egui::Response> {
    if !first {
        ui.add_space(4.0);
        let (line, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
        ui.painter().rect_filled(line, 0.0, theme().line());
    }
    let (rect, row) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::hover());
    ui.painter().text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(11.0),
        theme().ansi[8],
    );
    // Los grupos de un repo llevan un "+" a la derecha: abre otro agente en ese repo.
    let path = tip?;
    let plus_rect = egui::Rect::from_center_size(
        rect.right_center() - egui::vec2(14.0, 0.0),
        egui::vec2(20.0, 20.0),
    );
    let plus = ui.interact(plus_rect, row.id.with("mas"), egui::Sense::click());
    let color = if plus.hovered() {
        theme().fg
    } else {
        theme().ansi[8]
    };
    ui.painter().text(
        plus_rect.center(),
        egui::Align2::CENTER_CENTER,
        "+",
        egui::FontId::proportional(14.0),
        color,
    );
    row.on_hover_text(path.display().to_string());
    Some(plus.on_hover_text("Otro agente en este proyecto"))
}

impl Orion {
    /// Prepara la app cuando egui ya creó la ventana: la fuente y la primera pestaña.
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        grid: (u16, u16),
    ) -> Result<Self, Error> {
        apply_font(&cc.egui_ctx, &settings.font.family);
        menu::apply_theme(&cc.egui_ctx, &settings.theme);
        let first = Session::new(&cc.egui_ctx, grid, default_shell(&settings))?;
        let orion = Self {
            tabs: vec![Tab::new(first)],
            active: 0,
            settings,
            menu: SettingsMenu::default(),
            palette: Palette::default(),
            orchestrate: Default::default(),
            project: Default::default(),
            saved: project::load_saved(),
            focused: true,
            toast: None,
            window_title: String::new(),
            config_modified: config_modified(),
            config_checked: Instant::now(),
            perf: None,
            window: crate::backdrop::Window::of(cc),
            updater: Updater::new(),
            update_asked: false,
            logo: cc.egui_ctx.load_texture(
                "logo",
                egui::ColorImage::from_rgba_unmultiplied([64, 64], &crate::icon::render(64)),
                egui::TextureOptions::LINEAR,
            ),
            #[cfg(target_os = "macos")]
            mac_menu: macmenu::MacMenu::install(&cc.egui_ctx),
        };
        orion.apply_window_look();
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

    /// El panel con foco de la pestaña activa.
    fn session(&mut self) -> &mut Session {
        self.tabs[self.active].focused_mut()
    }

    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// Abre una pestaña nueva del mismo tamaño que la actual, y la deja activa.
    /// `kind`: qué shell (índice en `pty::SHELLS`); None = la de config.toml.
    fn new_tab(&mut self, ctx: &egui::Context, kind: Option<usize>) {
        let grid = self.session().grid;
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
        let cwd = self.session().cwd.clone();
        self.launch_agent(ctx, cwd, &agent.name, agent.worktree, |_| {
            agent.command.clone()
        });
    }

    /// Pestaña nueva con un agente de `agents::PROVIDERS`, en la carpeta actual.
    fn open_provider(&mut self, ctx: &egui::Context, index: usize) {
        let Some(provider) = crate::agents::PROVIDERS.get(index) else {
            return;
        };
        let cwd = self.session().cwd.clone();
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
            match crate::agents::create_worktree(&cwd, name) {
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
        let shell = &crate::pty::SHELLS[tab.focused().kind];
        let command = command(shell);
        // Una terminal sola (sin comando) no es un agente.
        tab.agent = (!command.is_empty()).then(|| name.to_string());
        tab.worktree = if worktree { dir.clone() } else { None };
        let input = crate::agents::launch_input(shell, dir.as_deref(), &command);
        tab.focused().write(input.as_bytes());
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

    /// Abre la ventana de "Nuevo proyecto", empezando en el repo de la pestaña actual si lo hay.
    fn open_project(&mut self) {
        let choices = self.agent_choices();
        self.project.open = true;
        self.project.error = None;
        // La carpeta de la vez anterior, o el repo de la pestaña actual; se vuelve a mirar si es repo.
        let cwd = self.session().cwd.clone();
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

    /// Cierra una pestaña (y las shells de todos sus paneles). Si era la última, se cierra la ventana.
    fn close_tab(&mut self, ctx: &egui::Context, index: usize) {
        let mut tab = self.tabs.remove(index);
        for session in tab.panes.leaves_mut() {
            let _ = session.shell.child.kill();
        }
        if self.tabs.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if tab.worktree.is_some() {
            self.show_toast("El worktree sigue en disco: paleta → «Limpiar worktrees de Orion»");
        }
        self.active = active_after_close(self.active, index, self.tabs.len());
    }

    /// Divide el panel con foco con una shell nueva, que queda con el foco.
    fn split(&mut self, ctx: &egui::Context, right: bool) {
        // El panel nuevo abre la misma shell, y se ajusta a su tamaño al dibujarse.
        let (grid, kind) = (self.session().grid, self.session().kind);
        match Session::new(ctx, grid, kind) {
            Ok(session) => {
                let id = session.id;
                let tab = &mut self.tabs[self.active];
                if tab.panes.split(tab.focus, right, session).is_ok() {
                    tab.focus = id;
                }
            }
            Err(e) => self.show_toast(format!("No pude abrir otro panel: {e}")),
        }
    }

    /// Cierra el panel `id` de la pestaña `index` (y su shell). Si era el único, se cierra la pestaña.
    fn close_pane(&mut self, ctx: &egui::Context, index: usize, id: u64) {
        let tab = &mut self.tabs[index];
        if tab.panes.leaves().len() == 1 {
            self.close_tab(ctx, index);
            return;
        }
        // El foco pasa al vecino más cercano: el de la izquierda, arriba, derecha o abajo.
        let dirs = [(-1.0, 0.0), (0.0, -1.0), (1.0, 0.0), (0.0, 1.0)];
        let next = dirs
            .into_iter()
            .find_map(|(x, y)| panes::neighbor(&tab.rects, id, egui::vec2(x, y)));
        if let Some(mut session) = tab.panes.remove(id) {
            let _ = session.shell.child.kill();
        }
        if tab.panes.get(tab.focus).is_none() {
            let first = tab.panes.leaves()[0].id;
            tab.focus = next
                .filter(|&n| tab.panes.get(n).is_some())
                .unwrap_or(first);
        }
    }

    /// Barra lateral de pestañas:
    /// "+ Nueva pestaña" arriba y una fila por pestaña con un punto de estado y el nombre de la carpeta.
    /// Se puede ensanchar o angostar arrastrando su borde.
    fn tab_sidebar(&self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        let frame = egui::Frame::new()
            .fill(surface(theme().sidebar()))
            .inner_margin(egui::Margin::symmetric(6, 8));
        let panel = egui::Panel::left("pestañas")
            .resizable(true)
            .default_size(190.0)
            .min_size(120.0)
            .max_size(360.0)
            // Monocromo (Vercel): una línea fina separa la barra de la terminal.
            .show_separator_line(theme().mono)
            .frame(frame);
        panel.show(ui, |ui| {
            let width = ui.available_width();
            let font = egui::FontId::proportional(13.0);

            // Fila "+ Nueva pestaña".
            let (rect, new) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
            if new.hovered() {
                ui.painter().rect_filled(rect, 6.0, theme().bg);
            }
            let color = if new.hovered() {
                theme().fg
            } else {
                theme().ansi[8]
            };
            let text_pos = rect.left_center() + egui::vec2(10.0, 0.0);
            ui.painter().text(
                text_pos,
                egui::Align2::LEFT_CENTER,
                "+  Nueva pestaña",
                font.clone(),
                color,
            );
            // Clic = terminal nueva. El menú (proyecto, otra shell, agente) abre con ▾ o clic derecho.
            let more_rect = egui::Rect::from_center_size(
                rect.right_center() - egui::vec2(14.0, 0.0),
                egui::vec2(20.0, 20.0),
            );
            let more = ui.interact(more_rect, new.id.with("menu"), egui::Sense::click());
            let more_color = if more.hovered() {
                theme().fg
            } else {
                theme().ansi[8]
            };
            ui.painter().text(
                more_rect.center(),
                egui::Align2::CENTER_CENTER,
                "▾",
                font.clone(),
                more_color,
            );
            if new.clicked() {
                action = Some(Action::NewTab(None));
            }
            let open = if new.secondary_clicked() {
                Some(egui::SetOpenCommand::Bool(true))
            } else if more.clicked() {
                Some(egui::SetOpenCommand::Toggle)
            } else {
                None
            };
            let providers = self.providers();
            egui::Popup::menu(&new).open_memory(open).show(|ui| {
                ui.weak("Proyecto");
                if ui
                    .button("Nuevo proyecto (repo + agente)…")
                    .on_hover_text("Elige un repo y un agente; con worktree, cada uno en su rama.")
                    .clicked()
                {
                    action = Some(Action::NewProject);
                    ui.close();
                }
                ui.separator();
                ui.weak("Terminal");
                let default = default_shell(&self.settings);
                if ui
                    .button(format!("Terminal ({})", SHELLS[default].name))
                    .clicked()
                {
                    action = Some(Action::NewTab(None));
                    ui.close();
                }
                for &i in installed_shells() {
                    if i != default && ui.button(format!("Terminal: {}", SHELLS[i].name)).clicked()
                    {
                        action = Some(Action::NewTab(Some(i)));
                        ui.close();
                    }
                }
                if !providers.is_empty() || !self.settings.agents.is_empty() {
                    ui.separator();
                    ui.weak("Agente en esta carpeta");
                }
                for &i in &providers {
                    let provider = &crate::agents::PROVIDERS[i];
                    let button = ui.button(format!("Agente: {}", provider.name));
                    if button.on_hover_text(provider.about).clicked() {
                        action = Some(Action::Provider(i));
                        ui.close();
                    }
                }
                for (i, agent) in self.settings.agents.iter().enumerate() {
                    if ui.button(format!("Agente: {}", agent.name)).clicked() {
                        action = Some(Action::Agent(i));
                        ui.close();
                    }
                }
            });
            let tip = keys!(
                "",
                "+Shift+T",
                "\n▾ o clic derecho: proyecto, shell o agente"
            );
            new.on_hover_text(tip);
            ui.add_space(6.0);

            // Pestañas agrupadas por repo: los repos en el orden en que aparecen y, al final,
            // las terminales sueltas. Sin ningún repo abierto la lista queda plana, como siempre.
            let has_repos = self.tabs.iter().any(|t| t.repo.is_some());
            let order = self.tab_order();
            let choices = self.agent_choices();
            let mut shown: Option<Option<&PathBuf>> = None;
            for i in order {
                let tab = &self.tabs[i];
                if has_repos && shown != Some(tab.repo.as_ref()) {
                    let label = tab.repo.as_deref().map_or("Terminales".into(), folder_name);
                    let plus =
                        group_header(ui, width, &label, tab.repo.as_deref(), shown.is_none());
                    if let Some(plus) = plus {
                        egui::Popup::menu(&plus).show(|ui| {
                            for (worktree, title) in [
                                (true, format!("Nueva rama en {label}")),
                                (false, "En la carpeta del repo".to_string()),
                            ] {
                                ui.weak(title);
                                for &choice in &choices {
                                    let name = choice.name(&self.settings);
                                    if ui.button(name).clicked() {
                                        action = Some(Action::InProject(i, choice, worktree));
                                        ui.close();
                                    }
                                }
                                if choices.is_empty() {
                                    ui.weak("No hay agentes");
                                }
                                ui.separator();
                            }
                            if ui.button("Terminal en la carpeta del repo").clicked() {
                                action = Some(Action::TerminalIn(i));
                                ui.close();
                            }
                        });
                    }
                    shown = Some(tab.repo.as_ref());
                }
                // Con splits, la fila muestra el panel con foco.
                let session = tab.focused();
                let (rect, row) =
                    ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
                let active = i == self.active;
                let painter = ui.painter();
                if active {
                    // La activa: fondo como el de la terminal y una línea del acento a la izquierda.
                    painter.rect_filled(rect, 6.0, theme().bg);
                    let accent = egui::Rect::from_min_size(
                        rect.min + egui::vec2(0.0, 7.0),
                        egui::vec2(3.0, 16.0),
                    );
                    painter.rect_filled(accent, 1.5, theme().accent());
                } else if row.hovered() {
                    painter.rect_filled(rect, 6.0, theme().hover());
                }

                // Punto de estado: cómo terminó el último comando (o amarillo si algo corre).
                // Si un panel de la pestaña pide atención, el punto es azul y late.
                let center = rect.left_center() + egui::vec2(14.0, 0.0);
                let alert = tab
                    .panes
                    .leaves()
                    .into_iter()
                    .find_map(|s| s.attention.clone());
                if alert.is_some() {
                    let t = ui.input(|i| i.time) as f32;
                    let pulse = (t * 4.0).sin() * 0.5 + 0.5; // de 0 a 1 y vuelta
                    let ring = theme().accent().gamma_multiply(0.35 * (1.0 - pulse));
                    painter.circle_filled(center, 4.0 + 4.0 * pulse, ring);
                    painter.circle_filled(center, 4.0, theme().accent());
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(50));
                } else {
                    let dot = session
                        .activity()
                        .map_or(theme().ansi[8], render::status_color);
                    painter.circle_filled(center, 3.5, dot);
                }

                // × a la derecha, solo al pasar el mouse por la fila.
                let close_rect = egui::Rect::from_center_size(
                    rect.right_center() - egui::vec2(14.0, 0.0),
                    egui::vec2(20.0, 20.0),
                );
                let close = ui.interact(close_rect, row.id.with("cerrar"), egui::Sense::click());

                // Nombre de la carpeta, recortado para que no pise la ×.
                let color = if active { theme().fg } else { theme().ansi[7] };
                let title_rect = egui::Rect::from_min_max(
                    rect.min + egui::vec2(26.0, 0.0),
                    egui::pos2(close_rect.min.x - 4.0, rect.max.y),
                );
                let galley = ui.painter().layout_no_wrap(
                    tab_label(tab.agent.as_deref(), &session.title()),
                    font.clone(),
                    color,
                );
                ui.painter().with_clip_rect(title_rect).galley(
                    title_rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0),
                    galley,
                    color,
                );

                if row.hovered() || close.hovered() {
                    let x_color = if close.hovered() {
                        theme().fg
                    } else {
                        theme().ansi[8]
                    };
                    ui.painter().text(
                        close_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "×",
                        font.clone(),
                        x_color,
                    );
                }
                if close.on_hover_text("Cerrar pestaña").clicked() {
                    action = Some(Action::CloseTab(i));
                }
                let mut tip = match session.last_command() {
                    Some(command) => {
                        format!("Último comando: {command}\nClic del medio para cerrar")
                    }
                    None => "Clic del medio para cerrar".into(),
                };
                if let Some(alert) = &alert {
                    tip = format!("🔔 {alert}\n{tip}");
                }
                let row = row.on_hover_text(tip);
                if row.clicked() {
                    action = Some(Action::SelectTab(i));
                }
                if row.middle_clicked() {
                    action = Some(Action::CloseTab(i));
                }
                ui.add_space(2.0);
            }

            // Proyectos guardados que no tienen pestaña abierta: un clic los reabre.
            let closed: Vec<usize> = (0..self.saved.len())
                .filter(|&i| !self.repo_busy(&self.saved[i].dir))
                .collect();
            if !closed.is_empty() {
                group_header(ui, width, "Guardados", None, self.tabs.is_empty());
            }
            for i in closed {
                let saved = &self.saved[i];
                let (rect, row) =
                    ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::click());
                if row.hovered() {
                    ui.painter().rect_filled(rect, 6.0, theme().hover());
                }
                let close_rect = egui::Rect::from_center_size(
                    rect.right_center() - egui::vec2(14.0, 0.0),
                    egui::vec2(20.0, 20.0),
                );
                let close = ui.interact(close_rect, row.id.with("olvidar"), egui::Sense::click());
                let title_rect = egui::Rect::from_min_max(
                    rect.min + egui::vec2(10.0, 0.0),
                    egui::pos2(close_rect.min.x - 4.0, rect.max.y),
                );
                let text = format!("{}  ·  {}", folder_name(&saved.dir), saved.agent);
                let color = theme().ansi[7];
                let galley = ui.painter().layout_no_wrap(text, font.clone(), color);
                ui.painter().with_clip_rect(title_rect).galley(
                    title_rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0),
                    galley,
                    color,
                );
                if row.hovered() || close.hovered() {
                    let x_color = if close.hovered() {
                        theme().fg
                    } else {
                        theme().ansi[8]
                    };
                    ui.painter().text(
                        close_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "×",
                        font.clone(),
                        x_color,
                    );
                }
                if close.on_hover_text("Quitar de los guardados").clicked() {
                    action = Some(Action::ForgetSaved(i));
                }
                if row.on_hover_text(saved.dir.display().to_string()).clicked() {
                    action = Some(Action::OpenSaved(i));
                }
                ui.add_space(2.0);
            }

            // Abajo de todo: el aviso de actualización, si hay una.
            let (text, next) = match self.updater.state() {
                UpdateState::Available(v) => (
                    format!("↑  Actualizar a {v}"),
                    Some(Action::InstallUpdate(v)),
                ),
                UpdateState::Installing(v) => (format!("Descargando {v}…"), None),
                UpdateState::Ready => ("↻  Reiniciar Orion".into(), Some(Action::RestartForUpdate)),
                _ => return,
            };
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                let (rect, row) =
                    ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::CLICK);
                if row.hovered() && next.is_some() {
                    ui.painter().rect_filled(rect, 6.0, theme().hover());
                }
                let pos = rect.left_center() + egui::vec2(10.0, 0.0);
                let color = theme().ansi[2];
                ui.painter()
                    .text(pos, egui::Align2::LEFT_CENTER, text, font.clone(), color);
                let tip = match next {
                    Some(Action::RestartForUpdate) => {
                        "Se cierran las pestañas y Orion abre la versión nueva"
                    }
                    Some(_) => "Descarga la versión nueva, la verifica y la deja lista",
                    None => "Puedes seguir usando Orion mientras tanto",
                };
                if row.on_hover_text(tip).clicked() {
                    action = next;
                }
            });
        });
        action
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
            Action::OpenSettings => self.open_menu(),
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
        for (t, tab) in self.tabs.iter_mut().enumerate() {
            let focus = tab.focus;
            for session in tab.panes.leaves_mut() {
                let Some(alert) = session.alert.take() else {
                    continue;
                };
                if !(focused && t == active && session.id == focus) {
                    session.attention = Some(alert);
                    new_alert = true;
                }
            }
        }
        // Lo que estás mirando ya no pide atención.
        if focused {
            self.session().attention = None;
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
        let menu_open = self.menu.open || self.palette.open || self.orchestrate.open;
        let tab = &mut self.tabs[self.active];
        // Entre panel y panel queda una línea de 2 px del color de la barra lateral.
        // Con Mica o acrílico no: los paneles semitransparentes se oscurecerían de más al pintarse encima.
        if opaque() {
            ui.painter().rect_filled(area, 0.0, theme().sidebar());
        }
        tab.rects.clear();
        tab.panes.layout(area, 2.0, &mut tab.rects);

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
            let Some(session) = tab.panes.get_mut(id) else {
                continue;
            };
            // Cada panel dibuja en su pedazo; id_salt separa los ids de egui de un panel y otro.
            let mut pane_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(id));
            let active = id == tab.focus;
            open_menu |= session.draw(&mut pane_ui, &self.settings.font, menu_open, active);
            if several && !active {
                // Los paneles sin foco, un poco más oscuros: se ve dónde vas a escribir.
                let dim = egui::Color32::from_black_alpha(60);
                ui.painter().rect_filled(rect, 0.0, dim);
            }
            if session.attention.is_some() {
                // Un panel de al lado pide atención: borde del acento.
                let stroke = egui::Stroke::new(2.0, theme().accent());
                ui.painter()
                    .rect_stroke(rect.shrink(1.0), 4.0, stroke, egui::StrokeKind::Inside);
            }
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
        // Se cerró la última pestaña y la ventana se está cerrando: este cuadro no tiene nada que hacer
        // (sin esto, el título de abajo buscaba la pestaña activa, que ya no existe, y Orion se caía).
        if self.tabs.is_empty() {
            return;
        }
        // Todos los paneles de todas las pestañas: los de atrás siguen recibiendo su salida.
        let mut pending = false;
        let mut copied = None;
        for tab in &mut self.tabs {
            for session in tab.panes.leaves_mut() {
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
            let mut sessions = tab.panes.leaves_mut().into_iter();
            sessions.find_map(|s| s.exited().then_some((t, s.id)))
        }) {
            self.close_pane(ctx, t, id);
            if self.tabs.is_empty() {
                return;
            }
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
        let title = format!("{} — Orion", self.session().title());
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if self.tabs.is_empty() {
            return; // la ventana se está cerrando
        }
        let ctx = ui.ctx().clone();

        // Si la app lo pidió (ESC[?1004h, ej. vim), avisarle cuando la ventana gana o pierde el foco.
        let focused = ui.input(|i| i.focused);
        if focused != self.focused {
            self.focused = focused;
            let session = self.session();
            if session.term.mode().contains(TermMode::FOCUS_IN_OUT) {
                session.write(if focused { b"\x1b[I" } else { b"\x1b[O" });
            }
        }

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
        if !others.is_empty() {
            let session = self.session();
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
            if self.tabs.is_empty() {
                return;
            }
        }

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
        } else if !self.menu.open && !self.palette.open && !self.orchestrate.open && !toggle {
            self.session().send_input(ui);
        }
        // Con el menú, la paleta o el orquestador abiertos, el teclado es para ellos, no para la shell.

        let area = ui.available_rect_before_wrap();
        if self.draw_panes(ui, area) {
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
        if let Some(subtasks) = self.orchestrate_window(&ctx) {
            self.launch_plan(&ctx, subtasks);
        }
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
    fn a_tab_names_its_agent_unless_the_title_already_does() {
        assert_eq!(tab_label(None, "benja"), "benja");
        assert_eq!(tab_label(Some("Codex"), "orion-1"), "Codex · orion-1");
        assert_eq!(tab_label(Some("OpenCode"), "OpenCode"), "OpenCode");
    }

    #[test]
    fn profile_choice_reads_the_agents_settings() {
        let mut settings = Settings::default();
        settings.agents.push(crate::config::Agent {
            name: "Mi agente".into(),
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
