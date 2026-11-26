//! Paleta de comandos (Ctrl+Shift+P; Cmd+Shift+P en Mac): buscar cualquier acción de Orion por su nombre.
//! Las entradas van agrupadas, con ícono, una línea que explica qué hacen y su atajo: así se aprenden
//! sin memorizarlos. Las listas largas (temas, fondos, shells) son subpáginas; al buscar aparece todo.

use super::chrome::{Icon, draw_icon_size, medium};
use super::overlay::{self, dim_text, faint_text, mix, tint};
use super::{Action, Orion};
use crate::colors::theme;
use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{self, Color32, FontId, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

const WIDTH: f32 = 660.0;
const ROW: f32 = 40.0;
const HEADER: f32 = 30.0;
const LIST_PAD: f32 = 6.0;
const MAX_LIST: f32 = 384.0;
const MAX_RECENT: usize = 3;

/// Una lista aparte, a la que se entra desde una fila.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Page {
    #[default]
    Root,
    Themes,
    Backdrops,
    Shells,
    Project,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Root => "",
            Page::Themes => "Tema",
            Page::Backdrops => "Fondo",
            Page::Shells => "Shell",
            Page::Project => "En el proyecto",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Page::Root => "Busca una acción, pestaña, agente o tema…",
            Page::Themes => "Busca un tema…",
            Page::Backdrops => "Busca un fondo…",
            Page::Shells => "Abrir una pestaña con…",
            Page::Project => "Qué abrir en este proyecto…",
        }
    }
}

/// Los grupos, en el orden en que se muestran.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Group {
    Start,
    Tabs,
    Agents,
    Projects,
    View,
    Appearance,
    System,
}

impl Group {
    fn name(self) -> &'static str {
        match self {
            Group::Start => "Empezar",
            Group::Tabs => "Pestañas",
            Group::Agents => "Agentes",
            Group::Projects => "Proyectos",
            Group::View => "Paneles y vista",
            Group::Appearance => "Apariencia",
            Group::System => "Sistema",
        }
    }
}

/// Los filtros de arriba (Tab pasa al siguiente) y los grupos que deja ver cada uno.
const FILTERS: [(&str, &[Group]); 5] = [
    ("Todo", &[]),
    ("Pestañas", &[Group::Tabs]),
    ("Agentes", &[Group::Agents]),
    ("Proyectos", &[Group::Projects]),
    ("Apariencia", &[Group::Appearance, Group::View]),
];

#[derive(Clone)]
enum Kind {
    Run(Action),
    Open(Page),
}

#[derive(Clone)]
struct Command {
    /// El nombre completo: con él se busca ("Tema: Gruvbox").
    name: String,
    /// Cómo se ve dentro de su subpágina ("Gruvbox"); vacío = el nombre.
    short: String,
    detail: String,
    keys: &'static str,
    icon: Icon,
    group: Group,
    /// La página donde vive. Las que no son de `Root` salen en la raíz solo al buscar.
    page: Page,
    kind: Kind,
    /// Por qué no se puede usar ahora; None = se puede.
    disabled: Option<&'static str>,
    /// Es la opción en uso (el tema o el fondo actual).
    current: bool,
    /// Colores de muestra (solo los temas).
    swatches: Option<[Color32; 4]>,
}

impl Command {
    fn new(group: Group, icon: Icon, name: impl Into<String>, detail: &str, kind: Kind) -> Self {
        Self {
            name: name.into(),
            short: String::new(),
            detail: detail.into(),
            keys: "",
            icon,
            group,
            page: Page::Root,
            kind,
            disabled: None,
            current: false,
            swatches: None,
        }
    }

    fn run(group: Group, icon: Icon, name: impl Into<String>, detail: &str, a: Action) -> Self {
        Self::new(group, icon, name, detail, Kind::Run(a))
    }

    fn keys(mut self, keys: &'static str) -> Self {
        self.keys = keys;
        self
    }

    fn inside(mut self, page: Page, short: impl Into<String>) -> Self {
        (self.page, self.short) = (page, short.into());
        self
    }

    fn disabled_if(mut self, off: bool, why: &'static str) -> Self {
        self.disabled = off.then_some(why);
        self
    }
}

#[derive(Default)]
pub(super) struct Palette {
    pub(super) open: bool,
    query: String,
    /// Fila resaltada (entre las que se muestran): Enter la ejecuta.
    selected: usize,
    page: Page,
    /// Índice en `FILTERS`.
    filter: usize,
    /// Los nombres de lo último que se ejecutó, de lo más nuevo a lo más viejo.
    recent: Vec<String>,
    /// La lista cambió (otro texto, filtro o página): el resaltado salta en vez de deslizarse
    /// y la lista vuelve arriba.
    jump: bool,
}

impl Palette {
    pub(super) fn toggle(&mut self) {
        let open = !self.open;
        if open {
            *self = Self {
                open,
                recent: std::mem::take(&mut self.recent),
                jump: true,
                ..Self::default()
            };
        } else {
            // Se deja todo como está: la salida se anima con lo que se estaba viendo.
            self.open = false;
        }
    }

    fn go(&mut self, page: Page) {
        self.page = page;
        self.query.clear();
        (self.selected, self.jump) = (0, true);
    }

    fn remember(&mut self, name: &str) {
        self.recent.retain(|n| n != name);
        self.recent.insert(0, name.into());
        self.recent.truncate(MAX_RECENT);
    }
}

fn fold_char(c: char) -> char {
    match c.to_lowercase().next().unwrap_or(c) {
        'á' => 'a',
        'é' => 'e',
        'í' => 'i',
        'ó' => 'o',
        'ú' | 'ü' => 'u',
        c => c,
    }
}

/// Minúsculas y sin tildes: "configuracion" encuentra "Configuración".
fn fold(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

/// Qué tan bien `name` coincide con `query` (menor = mejor), o None si no coincide.
/// 0: empieza así; 1: lo contiene; 2: tiene sus letras en orden ("ddere" → "Dividir a la derecha").
fn score(name: &str, query: &str) -> Option<u8> {
    let (name, query) = (fold(name), fold(query.trim()));
    if name.starts_with(&query) {
        return Some(0);
    }
    if name.contains(&query) {
        return Some(1);
    }
    let mut letters = name.chars();
    let mut wanted = query.chars().filter(|c| !c.is_whitespace());
    wanted.all(|q| letters.any(|c| c == q)).then_some(2)
}

/// Las posiciones (en caracteres) de `name` que coinciden con `query`, para resaltarlas.
fn matched(name: &str, query: &str) -> Vec<usize> {
    let name: Vec<char> = name.chars().map(fold_char).collect();
    let query: Vec<char> = query.trim().chars().map(fold_char).collect();
    if query.is_empty() {
        return Vec::new();
    }
    if let Some(start) = name.windows(query.len()).position(|w| w == query) {
        return (start..start + query.len()).collect();
    }
    let mut found = Vec::new();
    let mut from = 0;
    for q in query.into_iter().filter(|c| !c.is_whitespace()) {
        let Some(at) = name[from..].iter().position(|&c| c == q) else {
            return Vec::new();
        };
        found.push(from + at);
        from += at + 1;
    }
    found
}

/// Los índices de `names` que coinciden con `query`, de mejor a peor.
/// Si empatan, quedan en el orden de la lista.
fn ranked(names: &[&str], query: &str) -> Vec<usize> {
    let mut hits: Vec<(u8, usize)> = names
        .iter()
        .enumerate()
        .filter_map(|(i, name)| Some((score(name, query)?, i)))
        .collect();
    hits.sort();
    hits.into_iter().map(|(_, i)| i).collect()
}

/// Lo que se ve: títulos de grupo y filas (con el índice de su comando).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Item {
    Header(&'static str),
    Row(usize),
}

fn visible(commands: &[Command], p: &Palette) -> Vec<Item> {
    let groups = FILTERS[p.filter].1;
    let in_filter = |c: &Command| groups.is_empty() || groups.contains(&c.group);
    let searching = !p.query.trim().is_empty();
    let pool: Vec<usize> = (0..commands.len())
        .filter(|&i| {
            let c = &commands[i];
            if p.page != Page::Root {
                return c.page == p.page;
            }
            // En la raíz, lo de las subpáginas aparece al buscar o al filtrar por su grupo.
            in_filter(c) && (c.page == Page::Root || searching)
        })
        .collect();

    if searching {
        let names: Vec<&str> = pool.iter().map(|&i| commands[i].name.as_str()).collect();
        let hits = ranked(&names, &p.query);
        let mut items = Vec::with_capacity(hits.len() + 1);
        if !hits.is_empty() {
            items.push(Item::Header("Resultados"));
        }
        items.extend(hits.into_iter().map(|j| Item::Row(pool[j])));
        return items;
    }

    let mut items = Vec::new();
    if p.page == Page::Root && p.filter == 0 {
        let recent: Vec<usize> = (p.recent.iter())
            .filter_map(|name| commands.iter().position(|c| &c.name == name))
            .collect();
        if !recent.is_empty() {
            items.push(Item::Header("Recientes"));
            items.extend(recent.into_iter().map(Item::Row));
        }
    }
    let mut sorted = pool;
    sorted.sort_by_key(|&i| commands[i].group);
    let mut last = None;
    for i in sorted {
        if p.page == Page::Root && last != Some(commands[i].group) {
            last = Some(commands[i].group);
            items.push(Item::Header(commands[i].group.name()));
        }
        items.push(Item::Row(i));
    }
    items
}

/// El nombre con las letras buscadas resaltadas.
fn title_job(text: &str, query: &str, color: Color32, width: f32) -> LayoutJob {
    let hits = matched(text, query);
    let mut job = LayoutJob::default();
    for (i, c) in text.chars().enumerate() {
        let hit = hits.contains(&i);
        let base = if hits.is_empty() || hit {
            color
        } else {
            mix(color, overlay::panel_fill(), 0.38)
        };
        let format = TextFormat {
            font_id: medium(13.5),
            color: base,
            ..Default::default()
        };
        super::render::push_char(&mut job, c, format);
    }
    job.wrap = TextWrapping::truncate_at_width(width);
    job
}

fn draw_row(ui: &egui::Ui, rect: Rect, c: &Command, page: Page, query: &str, active: bool) {
    let off = c.disabled.is_some();
    let painter = ui.painter();
    let fg = if off {
        mix(theme().fg, overlay::panel_fill(), 0.62)
    } else {
        theme().fg
    };

    // El ícono, en su cajita: se ilumina en la fila elegida.
    let tile = Rect::from_center_size(pos2(rect.min.x + 24.0, rect.center().y), vec2(26.0, 26.0));
    if let Some(colors) = c.swatches {
        painter.rect(
            tile,
            7.0,
            colors[0],
            Stroke::new(1.0, tint(0.18)),
            StrokeKind::Inside,
        );
        for (n, color) in colors.iter().skip(1).enumerate() {
            let at = pos2(tile.min.x + 7.0 + 6.0 * n as f32, tile.center().y);
            painter.circle_filled(at, 2.4, *color);
        }
    } else {
        let lit = active && !off;
        painter.rect(
            tile,
            7.0,
            tint(if lit { 0.13 } else { 0.05 }),
            Stroke::new(1.0, tint(if lit { 0.22 } else { 0.09 })),
            StrokeKind::Inside,
        );
        let color = if off {
            fg
        } else if lit {
            theme().fg
        } else {
            dim_text()
        };
        draw_icon_size(ui, c.icon, tile.center(), color, 15.0);
    }

    // A la derecha: la flecha de subpágina, el atajo o la marca de "en uso".
    let mut right = rect.max.x - 12.0;
    if matches!(c.kind, Kind::Open(_)) {
        let at = pos2(right - 7.0, rect.center().y);
        let color = if active { theme().fg } else { faint_text() };
        draw_icon_size(ui, Icon::ChevronRight, at, color, 14.0);
        right -= 22.0;
    }
    if !c.keys.is_empty() {
        let color = if active { theme().fg } else { dim_text() };
        overlay::keys_at(ui, pos2(right, rect.center().y), c.keys, color);
        right -= overlay::keys_width(ui, c.keys) + 10.0;
    }
    if c.current {
        let at = pos2(right - 7.0, rect.center().y);
        draw_icon_size(ui, Icon::Check, at, theme().fg, 15.0);
        right -= 22.0;
    }

    let left = rect.min.x + 48.0;
    let text = if page != Page::Root && !c.short.is_empty() {
        &c.short
    } else {
        &c.name
    };
    let galley = ui
        .painter()
        .layout_job(title_job(text, query, fg, (right - left).max(0.0)));
    let width = galley.size().x;
    let pos = pos2(left, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, fg);

    let detail = c.disabled.unwrap_or(&c.detail);
    let area = Rect::from_min_max(
        pos2(left + width + 10.0, rect.min.y),
        pos2(right - 8.0, rect.max.y),
    );
    if !detail.is_empty() && area.width() > 40.0 {
        let color = if off { fg } else { faint_text() };
        overlay::line_at(
            ui.painter(),
            area,
            detail,
            FontId::proportional(12.0),
            color,
        );
    }
}

impl Orion {
    /// Todo lo que se puede hacer desde la paleta.
    fn commands(&self) -> Vec<Command> {
        use Group::*;
        let a = self.active;
        let no_tab = self.tabs.is_empty();
        let one_tab = self.tabs.len() < 2;
        const NO_TAB: &str = "No hay ninguna pestaña abierta";
        const ONE_TAB: &str = "Solo hay una pestaña";

        let mut list = vec![
            Command::run(
                Start,
                Icon::Plus,
                "Nueva pestaña",
                "Una terminal nueva",
                Action::NewTab(None),
            )
            .keys(keys!("+Shift+T")),
            Command::run(
                Start,
                Icon::Folder,
                "Nuevo proyecto…",
                "Elige un repo y los agentes que lo trabajan",
                Action::NewProject,
            )
            .keys(keys!("+Shift+O")),
            Command::run(
                Start,
                Icon::GitMerge,
                "Orquestar tarea con agentes…",
                "Divide una tarea y reparte cada parte a un agente",
                Action::Orchestrate,
            ),
            Command::new(
                Start,
                Icon::Terminal,
                "Nueva pestaña con otra shell…",
                "PowerShell, cmd, WSL…",
                Kind::Open(Page::Shells),
            ),
        ];

        for &i in crate::pty::installed_shells() {
            let shell = crate::pty::SHELLS[i].name;
            list.push(
                Command::run(
                    Start,
                    Icon::Terminal,
                    format!("Nueva pestaña: {shell}"),
                    "",
                    Action::NewTab(Some(i)),
                )
                .inside(Page::Shells, shell),
            );
        }

        // Alt+número en Windows, Cmd+número en Mac (ver `shortcuts`).
        const ALT: [&str; 9] = [
            "Alt+1", "Alt+2", "Alt+3", "Alt+4", "Alt+5", "Alt+6", "Alt+7", "Alt+8", "Alt+9",
        ];
        const CMD: [&str; 9] = [
            "Cmd+1", "Cmd+2", "Cmd+3", "Cmd+4", "Cmd+5", "Cmd+6", "Cmd+7", "Cmd+8", "Cmd+9",
        ];
        let numbers = if super::MAC { CMD } else { ALT };
        for (pos, i) in self.tab_order().into_iter().enumerate() {
            let tab = &self.tabs[i];
            let detail = tab.repo.as_deref().map(super::folder_name);
            let mut command = Command::run(
                Tabs,
                Icon::Terminal,
                tab.title(),
                detail.as_deref().unwrap_or(""),
                Action::SelectTab(i),
            )
            .keys(numbers.get(pos).copied().unwrap_or(""));
            command.current = i == a;
            list.push(command);
        }
        list.extend([
            Command::run(
                Tabs,
                Icon::ChevronRight,
                "Siguiente pestaña",
                "",
                Action::SelectTab(if no_tab { 0 } else { self.tab_step(1) }),
            )
            .keys("Ctrl+Tab")
            .disabled_if(one_tab, if no_tab { NO_TAB } else { ONE_TAB }),
            Command::run(
                Tabs,
                Icon::ArrowBackUp,
                "Pestaña anterior",
                "",
                Action::SelectTab(if no_tab { 0 } else { self.tab_step(-1) }),
            )
            .keys("Ctrl+Shift+Tab")
            .disabled_if(one_tab, if no_tab { NO_TAB } else { ONE_TAB }),
            Command::run(Tabs, Icon::Close, "Cerrar pestaña", "", Action::CloseTab(a))
                .disabled_if(no_tab, NO_TAB),
        ]);

        for i in self.providers() {
            let provider = &crate::agents::PROVIDERS[i];
            list.push(Command::run(
                Agents,
                Icon::Palette,
                format!("Agente: {}", provider.name),
                provider.about,
                Action::Provider(i),
            ));
        }
        for (i, agent) in self.settings.agents.iter().enumerate() {
            list.push(Command::run(
                Agents,
                Icon::Palette,
                format!("Agente: {}", agent.name),
                &agent.command,
                Action::Agent(i),
            ));
        }

        // Proyecto de la pestaña activa: una terminal u otro agente, sin tocar el mouse.
        // (`get`: en el inicio no hay pestañas.)
        if let Some(repo) = self.tabs.get(a).and_then(|t| t.repo.as_deref()) {
            let project = super::folder_name(repo);
            list.push(Command::new(
                Projects,
                Icon::Plus,
                format!("Abrir en {project}…"),
                "Un agente o una terminal en este repo",
                Kind::Open(Page::Project),
            ));
            list.push(Command::run(
                Projects,
                Icon::GitBranch,
                "Ver grafo de ramas",
                "Ramas y commits del repo",
                Action::Graph(a),
            ));
            list.push(
                Command::run(
                    Projects,
                    Icon::Terminal,
                    format!("Terminal en {project}"),
                    "Sin agente",
                    Action::TerminalIn(a),
                )
                .inside(Page::Project, "Terminal sola"),
            );
            for choice in self.agent_choices() {
                let name = choice.name(&self.settings).to_string();
                list.push(
                    Command::run(
                        Projects,
                        Icon::GitBranch,
                        format!("{name} en {project} (rama nueva)"),
                        "En su propio worktree",
                        Action::InProject(a, choice, true),
                    )
                    .inside(Page::Project, format!("{name} · rama nueva")),
                );
                list.push(
                    Command::run(
                        Projects,
                        Icon::Folder,
                        format!("{name} en {project} (carpeta del repo)"),
                        "Comparte la carpeta del repo",
                        Action::InProject(a, choice, false),
                    )
                    .inside(Page::Project, format!("{name} · carpeta del repo")),
                );
            }
        }
        // Proyectos guardados que no están abiertos.
        for (i, saved) in self.saved.iter().enumerate() {
            if !self.repo_busy(&saved.dir) {
                list.push(Command::run(
                    Projects,
                    Icon::Folder,
                    format!("Abrir proyecto: {}", super::folder_name(&saved.dir)),
                    &saved.agent,
                    Action::OpenSaved(i),
                ));
            }
        }
        list.push(Command::run(
            Projects,
            Icon::Trash,
            "Limpiar worktrees de Orion",
            "Borra los que ninguna pestaña usa y no tienen cambios",
            Action::CleanWorktrees,
        ));

        list.extend([
            Command::run(
                View,
                Icon::Split,
                "Dividir a la derecha",
                "",
                Action::Split(true),
            )
            .keys(keys!("+Shift+D"))
            .disabled_if(no_tab, NO_TAB),
            Command::run(
                View,
                Icon::ArrowBarToDown,
                "Dividir hacia abajo",
                "",
                Action::Split(false),
            )
            .keys(keys!("+Shift+E"))
            .disabled_if(no_tab, NO_TAB),
            Command::run(View, Icon::Close, "Cerrar panel", "", Action::ClosePane)
                .keys(keys!("+Shift+W"))
                .disabled_if(no_tab, NO_TAB),
            Command::run(
                View,
                Icon::Sidebar,
                "Barra lateral",
                if self.sidebar_hidden {
                    "Mostrarla"
                } else {
                    "Ocultarla"
                },
                Action::ToggleSidebar,
            )
            .keys(keys!("+B")),
        ]);

        list.extend([
            Command::new(
                Appearance,
                Icon::Theme,
                "Cambiar tema…",
                theme().name,
                Kind::Open(Page::Themes),
            ),
            Command::new(
                Appearance,
                Icon::Colors,
                "Cambiar fondo…",
                &self.settings.backdrop,
                Kind::Open(Page::Backdrops),
            ),
            Command::run(
                Appearance,
                Icon::Font,
                "Agrandar letra",
                "",
                Action::FontSize(1.0),
            ),
            Command::run(
                Appearance,
                Icon::Font,
                "Achicar letra",
                "",
                Action::FontSize(-1.0),
            ),
        ]);
        for (i, (name, _)) in crate::backdrop::BACKDROPS.iter().enumerate() {
            let mut command = Command::run(
                Appearance,
                Icon::Colors,
                format!("Fondo: {name}"),
                "",
                Action::Backdrop(i),
            )
            .inside(Page::Backdrops, *name);
            command.current = name.eq_ignore_ascii_case(&self.settings.backdrop);
            list.push(command);
        }
        for (i, t) in crate::colors::THEMES.iter().enumerate() {
            let mut command = Command::run(
                Appearance,
                Icon::Theme,
                format!("Tema: {}", t.name),
                if t.is_light() { "Claro" } else { "Oscuro" },
                Action::Theme(i),
            )
            .inside(Page::Themes, t.name);
            command.current = t.name.eq_ignore_ascii_case(&self.settings.theme);
            command.swatches = Some([t.bg, t.ansi[1], t.ansi[2], t.ansi[4]]);
            list.push(command);
        }

        list.extend([
            Command::run(
                System,
                Icon::Gear,
                "Configuración",
                "Temas, fuentes, shell y más",
                Action::OpenSettings,
            )
            .keys(keys!("+,")),
            Command::run(
                System,
                Icon::Advanced,
                "Mostrar rendimiento",
                "Milisegundos por frame",
                Action::TogglePerf,
            ),
            Command::run(
                System,
                Icon::Refresh,
                "Buscar actualizaciones",
                &format!("Tienes la {}", crate::update::CURRENT),
                Action::CheckUpdates,
            ),
        ]);
        list
    }

    /// Dibuja la paleta (abierta, o terminando de cerrarse). Devuelve la acción elegida (Enter o clic).
    pub(super) fn palette_window(&mut self, ctx: &egui::Context) -> Option<Action> {
        let open = self.palette.open;
        // Los comandos solo se arman mientras se ve (o se está yendo).
        let commands = self.commands();
        let p = &mut self.palette;

        // Flechas, Enter, Tab y Escape son de la lista: se consumen antes de que el cuadro de texto las vea.
        let none = egui::Modifiers::NONE;
        // Cuántas filas moverse: con la flecha apretada pueden llegar varias repeticiones en un frame.
        let (mut step, mut enter, mut escape) = (0isize, false, false);
        if open {
            let (tab, back_tab, backspace) = ctx.input_mut(|i| {
                step = i.count_and_consume_key(none, egui::Key::ArrowDown) as isize
                    - i.count_and_consume_key(none, egui::Key::ArrowUp) as isize;
                enter = i.consume_key(none, egui::Key::Enter);
                escape = i.consume_key(none, egui::Key::Escape);
                // Primero Shift+Tab: `consume_key` sin modificadores también lo tomaría.
                let back_tab = i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab);
                let tab = i.consume_key(none, egui::Key::Tab);
                let backspace = p.query.is_empty()
                    && p.page != Page::Root
                    && i.consume_key(none, egui::Key::Backspace);
                (tab, back_tab, backspace)
            });
            if p.page == Page::Root && (tab || back_tab) {
                let step = if back_tab { FILTERS.len() - 1 } else { 1 };
                p.filter = (p.filter + step) % FILTERS.len();
                (p.selected, p.jump) = (0, true);
            }
            // Escape primero sale de la subpágina; en la raíz, cierra.
            if backspace || (escape && p.page != Page::Root) {
                p.go(Page::Root);
                escape = false;
            }
        }

        let mut chosen = None;
        let mut rows: Vec<usize> = Vec::new();
        let shown = overlay::show(ctx, "palette", open, WIDTH, 0.12, |ui| {
            search_header(ui, p, open);
            if p.page == Page::Root {
                filter_bar(ui, p);
            }
            overlay::divider(ui);
            // La lista se arma recién acá, con el texto y el filtro de este mismo frame:
            // armada antes, los resultados iban un frame atrás de lo que escribías.
            let items = visible(&commands, p);
            rows = (items.iter())
                .filter_map(|item| match item {
                    Item::Row(i) => Some(*i),
                    Item::Header(_) => None,
                })
                .collect();
            let count = rows.len();
            if count > 0 {
                p.selected = (p.selected as isize + step).rem_euclid(count as isize) as usize;
            }
            p.selected = p.selected.min(count.saturating_sub(1));
            chosen = (enter && count > 0).then_some(p.selected);
            let before = p.selected;
            if let Some(row) = list(ui, p, &commands, &items, step != 0) {
                chosen = Some(row);
            }
            // El mouse movió el resaltado después de dibujarlo: hace falta otro frame.
            if p.selected != before {
                ui.ctx().request_repaint();
            }
            overlay::divider(ui);
            let selected = rows.get(p.selected).map(|&i| &commands[i]);
            footer(ui, p.page, selected);
        })?;

        if !open {
            return None;
        }
        if escape || shown.clicked_outside {
            p.open = false;
            return None;
        }
        let command = &commands[*rows.get(chosen?)?];
        if command.disabled.is_some() {
            return None;
        }
        match &command.kind {
            Kind::Open(page) => {
                p.go(*page);
                ctx.request_repaint();
                None
            }
            Kind::Run(action) => {
                p.open = false;
                p.remember(&command.name);
                Some(action.clone())
            }
        }
    }
}

/// La lupa, la página en la que estás y el texto.
fn search_header(ui: &mut egui::Ui, p: &mut Palette, open: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 54.0), Sense::hover());
    draw_icon_size(
        ui,
        Icon::Search,
        pos2(rect.min.x + 24.0, rect.center().y),
        dim_text(),
        18.0,
    );
    let mut left = rect.min.x + 46.0;
    if p.page != Page::Root {
        let name = p.page.name();
        let galley = ui
            .painter()
            .layout_no_wrap(name.into(), medium(12.0), theme().fg);
        let crumb = Rect::from_min_size(
            pos2(left, rect.center().y - 12.0),
            vec2(galley.size().x + 18.0, 24.0),
        );
        let back = ui.interact(crumb, ui.scope_id().with("crumb"), Sense::click());
        ui.painter()
            .rect_filled(crumb, 7.0, tint(if back.hovered() { 0.16 } else { 0.09 }));
        let pos = crumb.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, theme().fg);
        let back = back
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Volver (Esc o Retroceso)");
        if back.clicked() {
            p.go(Page::Root);
        }
        left = crumb.max.x + 10.0;
    }
    let esc = if p.page == Page::Root {
        "Esc"
    } else {
        "Retroceso"
    };
    overlay::keys_at(
        ui,
        pos2(rect.max.x - 14.0, rect.center().y),
        esc,
        faint_text(),
    );
    let area = Rect::from_min_max(
        pos2(left, rect.center().y - 13.0),
        pos2(
            rect.max.x - 24.0 - overlay::keys_width(ui, esc),
            rect.center().y + 13.0,
        ),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(area)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let hint = egui::RichText::new(p.page.hint())
        .size(16.0)
        .color(faint_text());
    let edit = child.add(
        egui::TextEdit::singleline(&mut p.query)
            .id_salt("palette-query")
            .hint_text(hint)
            .font(FontId::proportional(16.0))
            .frame(egui::Frame::NONE)
            .lock_focus(true)
            .desired_width(f32::INFINITY),
    );
    if edit.changed() {
        (p.selected, p.jump) = (0, true);
    }
    if open {
        edit.request_focus();
    }
}

fn filter_bar(ui: &mut egui::Ui, p: &mut Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
    let area = Rect::from_min_max(pos2(rect.min.x + 12.0, rect.min.y), rect.max);
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(area)
            .layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    bar.spacing_mut().item_spacing.x = 6.0;
    for (i, (name, _)) in FILTERS.iter().enumerate() {
        if overlay::chip(&mut bar, name, p.filter == i, true, false).clicked() {
            (p.filter, p.selected, p.jump) = (i, 0, true);
        }
    }
}

/// La lista con su resaltado, que se desliza de una fila a otra. Devuelve la fila clickeada.
fn list(
    ui: &mut egui::Ui,
    p: &mut Palette,
    commands: &[Command],
    items: &[Item],
    keyboard: bool,
) -> Option<usize> {
    let content: f32 = (items.iter())
        .map(|i| match i {
            Item::Header(_) => HEADER,
            Item::Row(_) => ROW,
        })
        .sum();
    let empty = items.is_empty();
    let wanted = if empty {
        120.0
    } else {
        content + LIST_PAD * 2.0
    };
    let max = MAX_LIST.min((ui.ctx().content_rect().height() * 0.55).max(160.0));
    // El alto acompaña a los resultados en vez de saltar.
    let height =
        ui.ctx()
            .animate_value_with_time(ui.scope_id().with("height"), wanted.min(max), 0.06);
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    child.set_clip_rect(area.intersect(ui.clip_rect()));

    if empty {
        let center = area.center();
        overlay::centered(
            child.painter(),
            center - vec2(0.0, 10.0),
            &format!("Sin resultados para «{}»", p.query.trim()),
            medium(14.0),
            theme().fg,
        );
        overlay::centered(
            child.painter(),
            center + vec2(0.0, 14.0),
            "Prueba con otra palabra o cambia el filtro con Tab",
            FontId::proportional(12.0),
            faint_text(),
        );
        p.jump = false;
        return None;
    }

    let jump = std::mem::take(&mut p.jump);
    let mut clicked = None;
    let mouse_moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("palette-list", p.page as u8))
        .max_height(height)
        .auto_shrink(false);
    if jump {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let (full, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), content + LIST_PAD * 2.0),
            Sense::hover(),
        );
        // Dónde cae la fila elegida, contando desde arriba del contenido.
        let mut y = LIST_PAD;
        let mut row = 0;
        let mut target = None;
        for item in items {
            match item {
                Item::Header(_) => y += HEADER,
                Item::Row(_) => {
                    if row == p.selected {
                        target = Some(y);
                    }
                    row += 1;
                    y += ROW;
                }
            }
        }
        if let Some(target) = target {
            // Con el teclado el resaltado va pegado a la tecla; con el mouse se desliza apenas.
            let time = if jump || keyboard { 0.0 } else { 0.05 };
            let at =
                ui.ctx()
                    .animate_value_with_time(ui.scope_id().with("highlight"), target, time);
            let highlight = Rect::from_min_size(
                pos2(full.min.x + 8.0, full.min.y + at),
                vec2(full.width() - 16.0, ROW),
            )
            .shrink2(vec2(0.0, 1.0));
            ui.painter().rect_filled(highlight, 9.0, tint(0.085));
            if keyboard {
                // Con un margen, para que el título del grupo no quede tapado.
                let place = Rect::from_min_size(
                    pos2(full.min.x, full.min.y + target - HEADER),
                    vec2(1.0, ROW + HEADER + LIST_PAD),
                );
                // Sin scroll suave: con la flecha apretada la lista se quedaba atrás.
                ui.scroll_to_rect_animation(place, None, egui::style::ScrollAnimation::none());
            }
        }

        let mut y = full.min.y + LIST_PAD;
        let mut row = 0;
        for item in items {
            match *item {
                Item::Header(name) => {
                    let text = Rect::from_min_max(
                        pos2(full.min.x + 20.0, y + 6.0),
                        pos2(full.max.x - 12.0, y + HEADER),
                    );
                    overlay::line_at(ui.painter(), text, name, medium(11.0), faint_text());
                    y += HEADER;
                }
                Item::Row(i) => {
                    let rect = Rect::from_min_size(
                        pos2(full.min.x + 8.0, y),
                        vec2(full.width() - 16.0, ROW),
                    );
                    let response =
                        ui.interact(rect, ui.scope_id().with(("row", row)), Sense::click());
                    // El mouse mueve el mismo resaltado que las flechas: nunca hay dos.
                    if response.hovered() && mouse_moved && !keyboard {
                        p.selected = row;
                    }
                    let command = &commands[i];
                    let cursor = if command.disabled.is_some() {
                        egui::CursorIcon::NotAllowed
                    } else {
                        egui::CursorIcon::PointingHand
                    };
                    if response.on_hover_cursor(cursor).clicked() {
                        clicked = Some(row);
                    }
                    if ui.is_rect_visible(rect) {
                        let active = p.selected == row;
                        draw_row(ui, rect, command, p.page, &p.query, active);
                    }
                    row += 1;
                    y += ROW;
                }
            }
        }
    });
    clicked
}

/// Abajo: qué teclas sirven ahora mismo.
fn footer(ui: &mut egui::Ui, page: Page, selected: Option<&Command>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
    let action = match selected {
        Some(c) if c.disabled.is_some() => "No disponible",
        Some(c) if matches!(c.kind, Kind::Open(_)) => "Entrar",
        _ => "Ejecutar",
    };
    let mut hints = vec![("↑+↓", "Navegar"), ("Enter", action)];
    if page == Page::Root {
        hints.push(("Tab", "Filtrar"));
        hints.push(("Esc", "Cerrar"));
    } else {
        hints.push(("Esc", "Volver"));
    }
    let mut x = rect.min.x + 14.0;
    for (keys, text) in hints {
        x += overlay::keys_width(ui, keys);
        overlay::keys_at(ui, pos2(x, rect.center().y), keys, dim_text());
        let galley =
            ui.painter()
                .layout_no_wrap(text.into(), FontId::proportional(11.5), faint_text());
        let pos = pos2(x + 6.0, rect.center().y - galley.size().y / 2.0);
        x += 6.0 + galley.size().x + 16.0;
        ui.painter().galley(pos, galley, faint_text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_search_ranks_matches() {
        let names = [
            "Nueva pestaña",
            "Dividir a la derecha",
            "Cerrar panel",
            "Configuración",
        ];
        assert_eq!(ranked(&names, ""), [0, 1, 2, 3]); // sin texto: todas, en orden
        assert_eq!(ranked(&names, "configuracion"), [3]); // sin tilde también
        assert_eq!(ranked(&names, "ddere"), [1]); // letras en orden
        // "pa": "Cerrar panel" lo contiene, "Nueva pestaña" solo tiene las letras en orden.
        assert_eq!(ranked(&names, "pa"), [2, 0]);
        assert!(ranked(&names, "xyz").is_empty());
    }

    #[test]
    fn matched_letters_are_the_ones_that_get_highlighted() {
        assert_eq!(matched("Configuración", "racion"), [7, 8, 9, 10, 11, 12]);
        assert_eq!(matched("Dividir a la derecha", "dde"), [0, 4, 14]);
        assert!(matched("Cerrar panel", "").is_empty());
        assert!(matched("Cerrar panel", "xyz").is_empty());
    }

    #[test]
    fn subpages_show_at_the_root_only_when_searching() {
        let command =
            |name: &str, group| Command::run(group, Icon::Plus, name, "", Action::TogglePerf);
        let commands = [
            command("Nueva pestaña", Group::Start),
            command("Tema: Dracula", Group::Appearance).inside(Page::Themes, "Dracula"),
        ];
        let mut p = Palette::default();
        assert_eq!(
            visible(&commands, &p),
            [Item::Header("Empezar"), Item::Row(0)]
        );
        p.query = "dracu".into();
        assert_eq!(
            visible(&commands, &p),
            [Item::Header("Resultados"), Item::Row(1)]
        );
        p.go(Page::Themes);
        assert_eq!(visible(&commands, &p), [Item::Row(1)]);
        // Lo ejecutado vuelve arriba, sin repetirse.
        p.go(Page::Root);
        p.remember("Nueva pestaña");
        p.remember("Nueva pestaña");
        assert_eq!(
            visible(&commands, &p)[..2],
            [Item::Header("Recientes"), Item::Row(0)]
        );
        assert_eq!(p.recent.len(), 1);
    }
}
