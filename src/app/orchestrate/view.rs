//! Cómo se ve el orquestador: un panel por pasos (describir, revisar el plan, en marcha).
//! Solo dibuja: lo que se pide vuelve como `Intent` y lo aplica `orchestrate_window`.

use super::super::chrome::{Icon, draw_icon_size, medium};
use super::super::overlay::{
    self, Button, Kind, border, dim_text, faint_text, mix, panel_fill, tint,
};
use super::graph::{self, Edge};
use super::{Orchestrate, Run, Step};
use crate::colors::theme;
use crate::config::Executor;
use crate::orchestrator::Subtask;
use eframe::egui::{
    self, Color32, CursorIcon, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2,
    vec2,
};
use std::collections::HashMap;
use std::time::Duration;

pub(super) const WIDTH: f32 = 720.0;
/// Cada cuánto se repinta mientras algo gira: la ventana entera, así que no al máximo.
const SPINNER_FRAME: std::time::Duration = std::time::Duration::from_millis(33);
const PAD: i8 = 20;

/// Lo que se pidió desde la ventana.
pub(super) enum Intent {
    Close,
    Plan,
    Launch,
    /// Volver a la tarea; durante la planificación, cancelarla.
    Back,
    Forget,
    Resume,
    Clean,
    /// Elegiste otro ejecutor para una subtarea: (índice, id).
    Pick(usize, String),
    /// Diste por terminada a mano la subtarea con ese índice.
    Finish(usize),
    Retry(usize),
    /// Ir a la pestaña del panel con ese id.
    Reveal(u64),
    /// Abrir Configuración → Agentes para agregar ejecutores.
    EditConfig,
}

/// Un ejecutor del catálogo y por qué no se puede usar (None = está instalado).
pub(super) type Member = (Executor, Option<String>);

/// Quién puede planificar: id y para qué conviene.
pub(super) const PLANNERS: [(&str, &str); 4] = [
    (
        "claude-opus-5-5",
        "Recomendado. El mejor reparto en tareas grandes o con partes que dependen entre sí.",
    ),
    (
        "claude-sonnet-5-5",
        "Más rápido y consume menos. Ideal para tareas acotadas.",
    ),
    (
        "claude-haiku-5-5",
        "El más económico. Úsalo cuando el reparto es evidente.",
    ),
    (
        "claude-fable-5-2",
        "El más potente, para tareas muy complejas. Puede no estar incluido en tu plan.",
    ),
];

/// Puntos de partida para no empezar con la caja vacía: nombre y texto.
const EXAMPLES: [(&str, &str); 4] = [
    (
        "Nueva función",
        "Añade la siguiente función, con sus tests:\n",
    ),
    (
        "Arreglar un error",
        "Encuentra la causa de este error, arréglalo y añade un test que lo cubra:\n",
    ),
    (
        "Revisar y probar",
        "Revisa el código en busca de errores y casos sin test, y corrige lo que encuentres en: ",
    ),
    (
        "Documentar",
        "Documenta lo que falta, sin cambiar el comportamiento, en: ",
    ),
];

/// "claude-opus-5-5" → "Opus 5.5"; los demás, su id.
pub(super) fn display_name(id: &str) -> String {
    let Some(rest) = id.strip_prefix("claude-") else {
        return id.to_string();
    };
    let mut parts = rest.split('-');
    let mut name: String = parts.next().unwrap_or_default().to_string();
    if let Some(first) = name.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    let version: Vec<&str> = parts.collect();
    if !version.is_empty() {
        name = format!("{name} {}", version.join("."));
    }
    name
}

/// La ronda (desde 1) en la que arranca cada subtarea: una después de las que necesita.
pub(super) fn rounds(subtasks: &[&Subtask]) -> Vec<usize> {
    let mut rounds: Vec<usize> = Vec::with_capacity(subtasks.len());
    for s in subtasks {
        let last = s.after.iter().filter_map(|&j| rounds.get(j)).max();
        rounds.push(last.map_or(1, |r| r + 1));
    }
    rounds
}

/// La ventana entera.
pub(super) fn panel(
    ui: &mut Ui,
    o: &mut Orchestrate,
    step: &Step,
    team: &[Member],
    activity: &HashMap<u64, String>,
    out: &mut Vec<Intent>,
) {
    let current = if !o.plan.is_empty() {
        2
    } else {
        match step {
            Step::Ready(..) => 1,
            _ => 0,
        }
    };
    header(ui, o, out);
    stepper(ui, current);
    overlay::divider(ui);

    // Con escala de UI alta (hasta 300 %) el alto en puntos baja: no hay que forzar 200 puntos o
    // el footer de Lanzar queda fuera de la pantalla.
    let room = (ui.ctx().content_rect().height() * 0.92 - 200.0).max(80.0);
    egui::ScrollArea::vertical()
        .id_salt(("orquestador", current))
        .max_height(room)
        .show(ui, |ui| {
            egui::Frame::new().inner_margin(PAD).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                if !o.plan.is_empty() {
                    running(ui, o, activity, out);
                } else {
                    match step {
                        Step::Writing => writing(ui, o, team, out),
                        Step::Planning => planning(ui, o, out),
                        Step::Ready(subtasks, offered, note) => {
                            review(ui, o, subtasks, offered, team, note, out)
                        }
                        Step::Failed(e) => failed(ui, e),
                    }
                }
            });
        });

    overlay::divider(ui);
    footer(ui, o, step, team, out);
}

fn header(ui: &mut Ui, o: &Orchestrate, out: &mut Vec<Intent>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 62.0), Sense::hover());
    let tile = Rect::from_center_size(pos2(rect.min.x + 36.0, rect.center().y), vec2(34.0, 34.0));
    ui.painter().rect(
        tile,
        9.0,
        tint(0.08),
        Stroke::new(1.0, tint(0.16)),
        StrokeKind::Inside,
    );
    draw_icon_size(ui, Icon::GitMerge, tile.center(), theme().fg, 18.0);

    let text = |y: f32| {
        Rect::from_min_max(
            pos2(tile.max.x + 12.0, y - 9.0),
            pos2(rect.max.x - 60.0, y + 9.0),
        )
    };
    overlay::line_at(
        ui.painter(),
        text(rect.center().y - 9.0),
        "Orquestador",
        medium(15.5),
        theme().fg,
    );
    // Solo el nombre del repo: la ruta completa queda al pasar el mouse.
    let repo = super::super::folder_name(&o.dir);
    let line = text(rect.center().y + 10.0);
    draw_icon_size(
        ui,
        Icon::Folder,
        pos2(line.min.x + 6.0, line.center().y),
        faint_text(),
        12.0,
    );
    let name = Rect::from_min_max(pos2(line.min.x + 17.0, line.min.y), line.max);
    overlay::line_at(
        ui.painter(),
        name,
        &repo,
        FontId::proportional(12.0),
        dim_text(),
    );
    ui.interact(line, ui.scope_id().with("repo"), Sense::hover())
        .on_hover_text(o.dir.display().to_string());

    let close = Rect::from_center_size(pos2(rect.max.x - 28.0, rect.center().y), vec2(28.0, 28.0));
    let mut corner = ui.new_child(egui::UiBuilder::new().max_rect(close));
    if overlay::icon_button(
        &mut corner,
        Icon::Close,
        "Cerrar (Esc). El plan se conserva mientras Orion siga abierto.",
    )
    .clicked()
    {
        out.push(Intent::Close);
    }
}

/// ① Tarea ─ ② Plan ─ ③ Seguimiento, con la línea que avanza.
fn stepper(ui: &mut Ui, current: usize) {
    const STEPS: [&str; 3] = ["Tarea", "Plan", "Seguimiento"];
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
    let inner = rect.shrink2(vec2(PAD as f32, 0.0));
    let each = (inner.width() - 16.0) / 3.0;
    let at = ui.ctx().animate_value_with_time(
        egui::Id::unique("orquestador-paso"),
        current as f32,
        0.12,
    );
    for (i, name) in STEPS.iter().enumerate() {
        let x = inner.min.x + i as f32 * (each + 8.0);
        let bar = Rect::from_min_size(pos2(x, inner.max.y - 9.0), vec2(each, 3.0));
        ui.painter().rect_filled(bar, 2.0, tint(0.09));
        let filled = (at - i as f32 + 1.0).clamp(0.0, 1.0);
        if filled > 0.0 {
            let part = Rect::from_min_size(bar.min, vec2(each * filled, 3.0));
            ui.painter().rect_filled(part, 2.0, tint(0.75));
        }
        let done = i < current;
        let color = if i == current {
            theme().fg
        } else if done {
            dim_text()
        } else {
            faint_text()
        };
        let circle = pos2(x + 8.0, inner.min.y + 13.0);
        if done {
            draw_icon_size(ui, Icon::Check, circle, dim_text(), 13.0);
        } else {
            ui.painter()
                .circle_stroke(circle, 7.5, Stroke::new(1.0, color));
            overlay::centered(
                ui.painter(),
                circle,
                &(i + 1).to_string(),
                medium(10.0),
                color,
            );
        }
        let text = Rect::from_min_max(
            pos2(x + 22.0, circle.y - 9.0),
            pos2(x + each, circle.y + 9.0),
        );
        overlay::line_at(ui.painter(), text, name, medium(12.0), color);
    }
}

/// El título de una sección, con una aclaración al lado.
fn section(ui: &mut Ui, title: &str, hint: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(medium(12.5)).color(theme().fg));
        ui.label(RichText::new(hint).size(11.5).color(faint_text()));
    });
}

fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(faint_text()));
}

/// Una tarjeta: fondo apenas elevado y borde que se aclara al pasar el mouse.
fn card<R>(ui: &mut Ui, accent: Option<Color32>, content: impl FnOnce(&mut Ui) -> R) -> R {
    let mut frame = egui::Frame::new()
        .fill(tint(0.028))
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .begin(ui);
    frame
        .content_ui
        .set_width(frame.content_ui.available_width());
    let inner = content(&mut frame.content_ui);
    let response = frame.allocate_space(ui);
    let hovered = ui.rect_contains_pointer(response.rect);
    if hovered {
        frame.frame.stroke.color = tint(0.20);
    }
    if let Some(color) = accent {
        frame.frame.stroke.color = mix(panel_fill(), color, if hovered { 0.6 } else { 0.4 });
    }
    frame.paint(ui);
    inner
}

fn writing(ui: &mut Ui, o: &mut Orchestrate, team: &[Member], out: &mut Vec<Intent>) {
    // Se consume antes de la caja: si no, ⌘/Ctrl+Enter también metería una línea.
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
        && plan_blocker(o, team).is_none()
    {
        out.push(Intent::Plan);
    }

    ui.label(
        RichText::new("¿Qué quieres lograr?")
            .font(medium(14.0))
            .color(theme().fg),
    );
    note(
        ui,
        "La tarea se divide en partes, tú apruebas el plan y cada agente trabaja en su propia \
         pestaña y rama.",
    );
    let id = egui::Id::unique("orquestador-tarea");
    let focused = ui.memory(|m| m.has_focus(id));
    let focus = ui
        .ctx()
        .animate_bool_with_time(id.with("foco"), focused, 0.08);
    egui::Frame::new()
        .fill(tint(0.028))
        .stroke(Stroke::new(1.0, tint(0.14 + 0.22 * focus)))
        .corner_radius(10.0)
        .inner_margin(12)
        .show(ui, |ui| {
            let hint = RichText::new(
                "Describe el resultado que esperas. Cuanto más concreto (archivos, \
                 comportamiento, cómo probarlo), mejor será el plan.",
            )
            .size(13.5)
            .color(faint_text());
            let edit = egui::TextEdit::multiline(&mut o.task)
                .id(id)
                .hint_text(hint)
                .font(FontId::proportional(13.5))
                .frame(egui::Frame::NONE)
                .desired_rows(4)
                .desired_width(f32::INFINITY);
            let edit = ui.add(edit);
            if ui.memory(|m| m.focused().is_none()) {
                edit.request_focus();
            }
        });
    ui.horizontal(|ui| {
        ui.set_height(26.0);
        note(ui, "Empezar con:");
        for (name, text) in EXAMPLES {
            if overlay::chip(ui, name, false, true, false).clicked() {
                // Lo que ya escribiste no se pierde: va después del encabezado.
                if !o.task.starts_with(text) {
                    o.task = format!("{text}{}", o.task);
                }
                ui.memory_mut(|m| m.request_focus(id));
            }
        }
    });

    section(
        ui,
        "Planificador",
        "analiza el repo y arma el plan, sin cambiar código",
    );
    ui.horizontal_wrapped(|ui| {
        for (i, (id, _)) in PLANNERS.iter().enumerate() {
            if overlay::chip(ui, &display_name(id), o.planner_model == i, true, false).clicked() {
                o.planner_model = i;
            }
        }
    });
    note(ui, PLANNERS[o.planner_model].1);

    section(ui, "Equipo", "quiénes pueden recibir subtareas");
    let on = |o: &Orchestrate, m: &Member| m.1.is_none() && !o.excluded.contains(&m.0.id);
    let active = team.iter().filter(|m| on(o, m)).count();
    let gap = 8.0;
    let width = (ui.available_width() - gap) / 2.0;
    for pair in team.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for member in pair {
                let selected = on(o, member);
                // El último que queda no se puede apagar: alguien tiene que hacer el trabajo.
                let locked = selected && active == 1;
                if member_card(ui, member, selected, locked, width).clicked()
                    && member.1.is_none()
                    && !locked
                {
                    if selected {
                        o.excluded.insert(member.0.id.clone());
                    } else {
                        o.excluded.remove(&member.0.id);
                    }
                }
            }
        });
    }
    ui.horizontal(|ui| {
        if Button::new("Añadir otro agente", Kind::Ghost)
            .icon(Icon::Plus)
            .small()
            .show(ui)
            .on_hover_text(
                "Abre Configuración → Agentes para sumar agentes al equipo \
                 (OpenCode, Codex, Gemini…).",
            )
            .clicked()
        {
            out.push(Intent::EditConfig);
        }
    });
}

fn member_card(
    ui: &mut Ui,
    (executor, problem): &Member,
    selected: bool,
    locked: bool,
    width: f32,
) -> egui::Response {
    let available = problem.is_none();
    let sense = if available {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 54.0), sense);
    let hovered = available && !locked && response.hovered();
    let hover = ui
        .ctx()
        .animate_bool_with_time(response.id.with("hover"), hovered, 0.06);
    let on = ui
        .ctx()
        .animate_bool_with_time(response.id.with("on"), selected, 0.08);
    let color = theme().fg;
    let fill = mix(tint(0.018 + 0.03 * on + 0.025 * hover), color, 0.045 * on);
    let stroke = mix(tint(0.10 + 0.10 * hover), color, 0.22 * on);
    let body = if response.is_pointer_button_down_on() && !locked {
        rect.shrink(1.0)
    } else {
        rect
    };
    ui.painter().rect(
        body,
        10.0,
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );

    let check =
        Rect::from_center_size(pos2(body.max.x - 19.0, body.min.y + 19.0), vec2(16.0, 16.0));
    if selected {
        ui.painter().rect_filled(check, 5.0, color);
        draw_icon_size(ui, Icon::Check, check.center(), panel_fill(), 13.0);
    } else if available {
        ui.painter()
            .rect_stroke(check, 5.0, Stroke::new(1.0, tint(0.28)), StrokeKind::Inside);
    }

    let faded = |c: Color32| {
        if available {
            c
        } else {
            mix(c, panel_fill(), 0.45)
        }
    };
    let line = |y: f32, right: f32| {
        Rect::from_min_max(
            pos2(body.min.x + 14.0, y - 9.0),
            pos2(body.max.x - right, y + 9.0),
        )
    };
    overlay::line_at(
        ui.painter(),
        line(body.min.y + 19.0, 34.0),
        &display_name(&executor.id),
        medium(13.0),
        faded(if selected { theme().fg } else { dim_text() }),
    );
    let (detail, detail_color) = match problem {
        Some(why) => (why.as_str(), faded(dim_text())),
        None => (executor.use_for.as_str(), faint_text()),
    };
    overlay::line_at(
        ui.painter(),
        line(body.min.y + 37.0, 10.0),
        detail,
        FontId::proportional(11.5),
        detail_color,
    );

    let tip = match problem {
        Some(why) => format!("No disponible: {why}"),
        None if locked => format!(
            "{}\n\nDebe quedar al menos un agente activo.",
            executor.use_for
        ),
        None => format!("{}\n\n{}", executor.use_for, executor.command),
    };
    let cursor = if !available || locked {
        CursorIcon::NotAllowed
    } else {
        CursorIcon::PointingHand
    };
    response.on_hover_cursor(cursor).on_hover_text(tip)
}

/// Por qué no se puede planificar todavía; None = se puede.
fn plan_blocker(o: &Orchestrate, team: &[Member]) -> Option<&'static str> {
    if o.task.trim().is_empty() {
        Some("Escribe la tarea para continuar")
    } else if !team
        .iter()
        .any(|m| m.1.is_none() && !o.excluded.contains(&m.0.id))
    {
        Some("No hay ningún agente disponible en el equipo")
    } else {
        None
    }
}

fn planning(ui: &mut Ui, o: &Orchestrate, out: &mut Vec<Intent>) {
    let _ = out;
    let time = ui.input(|i| i.time);
    let seconds = o.started.map_or(0, |s| s.elapsed().as_secs());
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
        spinner(ui, rect.center(), 11.0, theme().fg, time);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let planner = display_name(PLANNERS[o.planner_model].0);
            ui.label(
                RichText::new(format!("{planner} está analizando el repo"))
                    .font(medium(14.0))
                    .color(theme().fg),
            );
            note(
                ui,
                &format!(
                    "{}:{:02} · Suele tardar un par de minutos. Puedes cerrar esta ventana; seguirá en curso.",
                    seconds / 60,
                    seconds % 60
                ),
            );
        });
    });
    section(ui, "Tarea", "");
    card(ui, None, |ui| {
        ui.label(RichText::new(o.task.trim()).size(12.5).color(dim_text()));
    });
    ui.add_space(6.0);
    // Donde van a aparecer las subtareas, latiendo de a una.
    for (n, width) in [0.46, 0.62, 0.38].into_iter().enumerate() {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
        let wave = ((time * 2.4 - n as f64 * 0.7).sin() * 0.5 + 0.5) as f32;
        ui.painter().rect(
            rect,
            10.0,
            tint(0.015 + 0.02 * wave),
            Stroke::new(1.0, tint(0.08)),
            StrokeKind::Inside,
        );
        let ghost = tint(0.06 + 0.06 * wave);
        ui.painter()
            .circle_filled(pos2(rect.min.x + 22.0, rect.center().y), 9.0, ghost);
        let bar = Rect::from_min_size(
            pos2(rect.min.x + 44.0, rect.center().y - 5.0),
            vec2(rect.width() * width, 10.0),
        );
        ui.painter().rect_filled(bar, 5.0, ghost);
    }
    ui.ctx().request_repaint_after(SPINNER_FRAME);
}

/// Un arco que gira.
fn spinner(ui: &Ui, center: Pos2, radius: f32, color: Color32, time: f64) {
    ui.painter()
        .circle_stroke(center, radius, Stroke::new(2.0, tint(0.10)));
    let start = (time * 4.5) as f32;
    let sweep = 1.8 + 0.9 * (time * 2.0).sin() as f32;
    let points: Vec<Pos2> = (0..=24)
        .map(|i| {
            let angle = start + sweep * i as f32 / 24.0;
            center + radius * vec2(angle.cos(), angle.sin())
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(points, Stroke::new(2.0, color)));
}

/// El número de una subtarea en su círculo. Devuelve dónde quedó.
fn number(ui: &mut Ui, n: usize) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
    ui.painter().circle(
        rect.center(),
        10.5,
        tint(0.06),
        Stroke::new(1.0, tint(0.16)),
    );
    overlay::centered(
        ui.painter(),
        rect.center(),
        &n.to_string(),
        medium(11.0),
        theme().fg,
    );
    rect
}

/// Ancho de cada carril del grafo, y lo que queda entre el último y las tarjetas.
const LANE: f32 = 12.0;
const LANE_GAP: f32 = 6.0;

/// Cómo se dibuja una línea del grafo.
enum Line {
    Solid(Color32),
    /// Punteada y en movimiento: la dependencia sigue trabajando.
    Moving(Color32),
}

/// La línea que sale de una subtarea lanzada, según cómo va.
fn line(run: &Run, title: &str) -> Line {
    let (_, color, why) = status(run, title);
    match run {
        _ if why.is_some() => Line::Solid(color),
        Run::Working { .. } => Line::Moving(color),
        Run::Waiting => Line::Solid(color.gamma_multiply(0.5)),
        _ => Line::Solid(color),
    }
}

/// El grafo de dependencias (al estilo `git log --graph`) en el margen izquierdo de las tarjetas.
/// Mientras se pintan se anota dónde quedó cada una; las líneas van al final, encima.
struct Graph {
    edges: Vec<Edge>,
    lanes: usize,
    len: usize,
}

/// La `Ui` de las tarjetas (corrida a la derecha del margen) y, por subtarea, el centro de su
/// círculo y el rect de su tarjeta.
type Cards = (Ui, Vec<(Pos2, Rect)>);

/// El rect de la tarjeta que se acaba de pintar, que empezó en `top`.
fn card_rect(ui: &Ui, top: f32) -> Rect {
    Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.min_rect().max.y)
}

impl Graph {
    fn new(subtasks: &[&Subtask], rounds: &[usize]) -> Self {
        let (edges, lanes) = graph::edges(subtasks, &graph::screen_order(rounds));
        let len = subtasks.len();
        Self { edges, lanes, len }
    }

    /// Reserva el margen: las tarjetas se pintan en la `Ui` que devuelve.
    fn margin(&self, ui: &mut Ui) -> Cards {
        let margin = if self.lanes == 0 {
            0.0
        } else {
            self.lanes as f32 * LANE + LANE_GAP
        };
        let mut rect = ui.available_rect_before_wrap();
        rect.min.x += margin;
        let cards = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        (cards, vec![(Pos2::ZERO, Rect::NOTHING); self.len])
    }

    /// Da por pintadas las tarjetas y dibuja las líneas; `style(origen)` dice cómo va cada una.
    fn finish(&self, ui: &mut Ui, (cards, at): Cards, style: impl Fn(usize) -> Line) {
        ui.advance_cursor_after_rect(cards.min_rect());
        // Al pasar el mouse por una tarjeta se destacan ella, sus vecinas y las líneas entre ellas.
        let hovered = at.iter().position(|(_, r)| ui.rect_contains_pointer(*r));
        let near = |i: usize| {
            hovered.is_none_or(|h| {
                i == h
                    || (self.edges.iter())
                        .any(|e| (e.from, e.to) == (h, i) || (e.to, e.from) == (h, i))
            })
        };
        let painter = ui.painter();
        for (i, (_, rect)) in at.iter().enumerate() {
            if !near(i) {
                painter.rect_filled(*rect, 10.0, panel_fill().gamma_multiply(0.55));
            }
        }

        let time = ui.input(|i| i.time) as f32;
        let mut moving = false;
        let left = cards.max_rect().min.x;
        for e in &self.edges {
            let (Some(&(a, _)), Some(&(b, _))) = (at.get(e.from), at.get(e.to)) else {
                continue;
            };
            let lit = hovered.is_some_and(|h| h == e.from || h == e.to);
            let (color, dashed) = match style(e.from) {
                Line::Solid(c) => (c, false),
                Line::Moving(c) => (c, true),
            };
            let (color, width) = match hovered {
                None => (color, 1.5),
                Some(_) if lit => (color, 2.0),
                Some(_) => (color.gamma_multiply(0.25), 1.5),
            };
            let stroke = Stroke::new(width, color);
            let points = path(a, b, left - LANE_GAP - LANE / 2.0 - e.lane as f32 * LANE);
            if dashed {
                moving = true;
                let offset = (time * 16.0).rem_euclid(8.0);
                painter.extend(egui::Shape::dashed_line_with_offset(
                    &points,
                    stroke,
                    &[4.0],
                    &[4.0],
                    offset,
                ));
            } else {
                painter.add(egui::Shape::line(points, stroke));
            }
        }
        if moving {
            ui.ctx().request_repaint_after(SPINNER_FRAME);
        }
    }
}

/// El trazo de una línea del grafo: sale del círculo `a` hacia la izquierda, baja (o sube) por el
/// carril en `x` con las esquinas curvas y entra al círculo `b`.
fn path(a: Pos2, b: Pos2, x: f32) -> Vec<Pos2> {
    // Del borde de cada círculo, no de su centro.
    let (a, b) = (a - vec2(11.0, 0.0), b - vec2(11.0, 0.0));
    let r = 7.0_f32.min((b.y - a.y).abs() / 2.0);
    let dy = r * (b.y - a.y).signum();
    let corner = |from: Pos2, at: Pos2, to: Pos2| {
        egui::epaint::QuadraticBezierShape::from_points_stroke(
            [from, at, to],
            false,
            Color32::TRANSPARENT,
            Stroke::NONE,
        )
        .flatten(Some(0.2))
    };
    let mut points = vec![a];
    points.extend(corner(pos2(x + r, a.y), pos2(x, a.y), pos2(x, a.y + dy)));
    points.extend(corner(pos2(x, b.y - dy), pos2(x, b.y), pos2(x + r, b.y)));
    points.push(b);
    points
}

/// El botón que muestra el ejecutor de una subtarea y deja cambiarlo.
fn executor_menu(
    ui: &mut Ui,
    i: usize,
    current: &str,
    offered: &[String],
    team: &[Member],
    out: &mut Vec<Intent>,
) {
    let name = display_name(current);
    let galley = ui.painter().layout_no_wrap(name, medium(12.0), theme().fg);
    let (rect, response) =
        ui.allocate_exact_size(vec2(galley.size().x + 36.0, 28.0), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        response.hovered() || open,
        0.06,
    );
    ui.painter().rect(
        rect,
        8.0,
        tint(0.055 + 0.055 * hover),
        Stroke::new(1.0, tint(0.13 + 0.12 * hover)),
        StrokeKind::Inside,
    );
    let pos = pos2(rect.min.x + 12.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, theme().fg);
    draw_icon_size(
        ui,
        Icon::Chevron,
        pos2(rect.max.x - 13.0, rect.center().y),
        mix(faint_text(), theme().fg, hover),
        13.0,
    );
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Cambiar el agente de esta subtarea");

    egui::Popup::menu(&response)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(6.0)
        .show(|ui| {
            ui.set_width(330.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            for id in offered {
                let use_for = (team.iter())
                    .find(|m| m.0.id == *id)
                    .map_or("", |m| m.0.use_for.as_str());
                let (rect, row) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                let selected = id == current;
                if selected || row.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        7.0,
                        tint(if row.hovered() { 0.11 } else { 0.07 }),
                    );
                }
                let line = |y: f32| {
                    Rect::from_min_max(
                        pos2(rect.min.x + 12.0, y - 9.0),
                        pos2(rect.max.x - 28.0, y + 9.0),
                    )
                };
                overlay::line_at(
                    ui.painter(),
                    line(rect.min.y + 15.0),
                    &display_name(id),
                    medium(12.5),
                    theme().fg,
                );
                overlay::line_at(
                    ui.painter(),
                    line(rect.min.y + 32.0),
                    use_for,
                    FontId::proportional(11.0),
                    faint_text(),
                );
                if selected {
                    let at = pos2(rect.max.x - 15.0, rect.center().y);
                    draw_icon_size(ui, Icon::Check, at, theme().fg, 14.0);
                }
                if row
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text(use_for)
                    .clicked()
                {
                    out.push(Intent::Pick(i, id.clone()));
                    ui.close();
                }
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn review(
    ui: &mut Ui,
    o: &mut Orchestrate,
    subtasks: &[Subtask],
    offered: &[String],
    team: &[Member],
    availability: &str,
    out: &mut Vec<Intent>,
) {
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)) {
        out.push(Intent::Launch);
    }
    let refs: Vec<&Subtask> = subtasks.iter().collect();
    let rounds = rounds(&refs);
    let total = rounds.iter().copied().max().unwrap_or(1);
    let graph = Graph::new(&refs, &rounds);
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    ui.label(
        RichText::new(format!(
            "{} en {}",
            plural(subtasks.len(), "subtarea", "subtareas"),
            plural(total, "ronda", "rondas")
        ))
        .font(medium(14.0))
        .color(theme().fg),
    );
    note(
        ui,
        "Cada subtarea abre una pestaña con su propia rama. Puedes cambiar su agente antes de \
         lanzar.",
    );

    let mut cards = graph.margin(ui);
    let (parent, ui) = (ui, &mut cards.0);
    for round in 1..=total {
        let members: Vec<usize> = (0..subtasks.len())
            .filter(|&i| rounds[i] == round)
            .collect();
        let title = format!("Ronda {round}");
        let hint = match (round, members.len()) {
            (1, 1) => "empieza al lanzar",
            (1, _) => "empiezan juntas al lanzar",
            (_, 1) => "empieza sola cuando esté listo lo que necesita",
            _ => "empiezan solas cuando esté listo lo que necesitan",
        };
        section(ui, &title, hint);
        for i in members {
            let s = &subtasks[i];
            let top = ui.cursor().min.y;
            card(ui, None, |ui| {
                let open = o.expanded == Some(i);
                ui.horizontal(|ui| {
                    ui.set_height(28.0);
                    cards.1[i].0 = number(ui, i + 1).center();
                    ui.add_space(2.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        executor_menu(ui, i, &s.executor, offered, team, out);
                        let text = if open { "Ocultar prompt" } else { "Ver prompt" };
                        if Button::new(text, Kind::Ghost).small().show(ui).clicked() {
                            o.expanded = (!open).then_some(i);
                        }
                        if !s.after.is_empty() {
                            overlay::badge(ui, &super::after_text(&s.after), dim_text());
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let title =
                                RichText::new(&s.title).font(medium(13.5)).color(theme().fg);
                            ui.add(egui::Label::new(title).truncate());
                        });
                    });
                });
                if open {
                    ui.add_space(4.0);
                    egui::Frame::new()
                        .fill(panel_fill())
                        .stroke(Stroke::new(1.0, border()))
                        .corner_radius(8.0)
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(&s.prompt).size(12.5).color(dim_text()));
                        });
                }
            });
            cards.1[i].1 = card_rect(ui, top);
        }
    }
    graph.finish(parent, cards, |_| Line::Solid(dim_text()));
    let ui = parent;
    ui.add_space(4.0);
    note(
        ui,
        "Al lanzar, esta ventana se cierra. Para seguir el avance, vuelve a abrir el orquestador.",
    );
    if !availability.is_empty() {
        note(ui, availability);
    }
}

fn failed(ui: &mut Ui, error: &str) {
    card(ui, Some(theme().ansi[1]), |ui| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
            draw_icon_size(
                ui,
                Icon::AlertTriangle,
                rect.center(),
                theme().ansi[1],
                18.0,
            );
            ui.label(
                RichText::new("No se pudo crear el plan")
                    .font(medium(14.0))
                    .color(theme().fg),
            );
        });
        ui.add(egui::Label::new(RichText::new(error).size(12.5).color(dim_text())).wrap());
    });
    note(ui, "Tu texto se conserva. Puedes reintentar o editarlo.");
}

/// La primera letra en mayúscula: los motivos se escriben para ir en medio de una frase.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// El estado de una subtarea lanzada: la etiqueta corta, su color y el motivo si hay problema.
fn status(run: &Run, title: &str) -> (&'static str, Color32, Option<String>) {
    let (text, bad) = run.label(title);
    match run {
        _ if bad => (
            "Necesita atención",
            theme().ansi[1],
            Some(capitalized(&text)),
        ),
        Run::Waiting => ("En espera", dim_text(), None),
        Run::Working { .. } => ("Trabajando", theme().fg, None),
        _ => ("Lista", theme().fg, None),
    }
}

/// La primera ronda con alguna subtarea que no terminó bien; `None` si todas lo hicieron.
fn active_round(rounds: &[usize], runs: &[&Run]) -> Option<usize> {
    (rounds.iter().zip(runs))
        .filter(|(_, run)| !run.succeeded())
        .map(|(&round, _)| round)
        .min()
}

/// Cuánto lleva trabajando un agente: "12 s", "3 min", "1 h 5 min".
fn elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    match (secs / 3600, secs / 60 % 60) {
        (0, 0) => format!("{secs} s"),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Si una fila de la terminal dice algo: no solo blancos ni dibujo de cajas.
pub(super) fn meaningful(row: &str) -> bool {
    row.chars()
        .any(|c| !c.is_whitespace() && !('\u{2500}'..='\u{257F}').contains(&c))
}

fn running(ui: &mut Ui, o: &Orchestrate, activity: &HashMap<u64, String>, out: &mut Vec<Intent>) {
    if o.paused {
        note(
            ui,
            "Plan recuperado, en pausa. Revisa los commits y pulsa Reanudar.",
        );
    }
    let time = ui.input(|i| i.time);
    let done = (o.plan.iter())
        .filter(|(s, run)| matches!(run, Run::Done { .. }) && !run.label(&s.title).1)
        .count();
    let total = o.plan.len();
    ui.label(
        RichText::new(format!("{done} de {total} subtareas listas"))
            .font(medium(14.0))
            .color(theme().fg),
    );
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    ui.painter().rect_filled(bar, 3.0, tint(0.09));
    let part = ui.ctx().animate_value_with_time(
        egui::Id::unique("orquestador-progreso"),
        done as f32 / total.max(1) as f32,
        0.2,
    );
    if part > 0.0 {
        let filled = Rect::from_min_size(bar.min, vec2(bar.width() * part, bar.height()));
        ui.painter().rect_filled(filled, 3.0, tint(0.75));
    }
    if let Some((s, _)) = o.plan.iter().find(|(_, run)| matches!(run, Run::Waiting)) {
        note(ui, &format!("Siguiente: {}", s.title));
    }
    note(
        ui,
        "Cuando un agente acabe, revisa su trabajo, confirma que hizo commit y escribe /exit. \
         Las que dependen de él empiezan solas.",
    );
    note(
        ui,
        "Al final, integra tú las ramas: Orion no las fusiona en tu rama principal.",
    );
    ui.add_space(4.0);

    let refs: Vec<&Subtask> = o.plan.iter().map(|(s, _)| s).collect();
    let runs: Vec<&Run> = o.plan.iter().map(|(_, run)| run).collect();
    let rounds = rounds(&refs);
    let active = active_round(&rounds, &runs);
    let graph = Graph::new(&refs, &rounds);
    let mut cards = graph.margin(ui);
    let (parent, ui) = (ui, &mut cards.0);

    let mut working = false;
    for round in 1..=rounds.iter().copied().max().unwrap_or(0) {
        let hint = match active {
            Some(active) if round == active => "en curso",
            Some(active) if round > active => "a continuación",
            _ => "terminada",
        };
        section(ui, &format!("Ronda {round}"), hint);
        for (i, (s, run)) in o.plan.iter().enumerate() {
            if rounds[i] != round {
                continue;
            }
            let (label, color, why) = status(run, &s.title);
            let accent = why.is_some().then_some(color);
            let top = ui.cursor().min.y;
            card(ui, accent, |ui| {
                ui.horizontal(|ui| {
                    ui.set_height(28.0);
                    if matches!(run, Run::Working { .. }) {
                        working = true;
                        let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                        spinner(ui, rect.center(), 8.0, color, time);
                        cards.1[i].0 = rect.center();
                    } else {
                        cards.1[i].0 = number(ui, i + 1).center();
                    }
                    ui.add_space(2.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        overlay::badge(ui, label, color);
                        if let Run::Working { branch, .. } = run
                            && o.committed.contains(branch)
                        {
                            overlay::badge(ui, "Ya hizo commit", theme().ansi[3]);
                        }
                        // «Reintentar» solo vuelve a lanzar una bloqueada. A una que ya terminó o se
                        // cerró no la ejecuta de nuevo: mira otra vez sus commits, y así se llama.
                        let (text, tip) = if matches!(run, Run::Blocked(_)) {
                            (
                                "Reintentar",
                                "Intenta arrancarla de nuevo cuando estén listas las que necesita",
                            )
                        } else {
                            (
                                "Revisar commits",
                                "No vuelve a ejecutar el agente: da por bueno su cierre y comprueba si \
                                 su rama tiene commits",
                            )
                        };
                        if run.can_retry()
                            && Button::new(text, Kind::Secondary)
                                .icon(Icon::Refresh)
                                .small()
                                .show(ui)
                                .on_hover_text(tip)
                                .clicked()
                        {
                            out.push(Intent::Retry(i));
                        }
                        if let Run::Working { pane, .. } = run {
                            // Plan B si la shell no avisa cuándo termina el agente (cmd, WSL).
                            if Button::new("Marcar terminada", Kind::Ghost)
                                .small()
                                .show(ui)
                                .on_hover_text(
                                    "Úsalo si Orion no detectó que el agente cerró (cmd, WSL). Se \
                                     comprueban sus commits.",
                                )
                                .clicked()
                            {
                                out.push(Intent::Finish(i));
                            }
                            if Button::new("Ver pestaña", Kind::Secondary)
                                .icon(Icon::ExternalLink)
                                .small()
                                .show(ui)
                                .clicked()
                            {
                                out.push(Intent::Reveal(*pane));
                            }
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let title = RichText::new(&s.title).font(medium(13.5)).color(theme().fg);
                            ui.add(egui::Label::new(title).truncate())
                                .on_hover_text(&s.prompt);
                        });
                    });
                });
                ui.horizontal(|ui| {
                    ui.add_space(32.0);
                    ui.label(
                        RichText::new(display_name(&s.executor))
                            .size(11.5)
                            .color(dim_text()),
                    );
                    if let Run::Working { pane, .. } = run
                        && let Some(since) = o.since.get(pane)
                    {
                        note(ui, &format!("·  {}", elapsed(since.elapsed())));
                    }
                    let detail = match run {
                        Run::Working { branch, .. }
                        | Run::Done { branch, .. }
                        | Run::Closed { branch, .. } => Some(branch.clone()),
                        Run::Waiting if !s.after.is_empty() => Some(super::after_text(&s.after)),
                        _ => None,
                    };
                    if let Some(detail) = detail {
                        note(ui, &format!("·  {detail}"));
                    }
                });
                if let Run::Working { pane, .. } = run
                    && let Some(row) = activity.get(pane)
                {
                    ui.horizontal(|ui| {
                        ui.add_space(32.0);
                        let text = RichText::new(row).size(11.0).color(dim_text());
                        ui.add(egui::Label::new(text).truncate());
                    });
                }
                if let Some(why) = why {
                    ui.horizontal(|ui| {
                        ui.add_space(32.0);
                        ui.add(egui::Label::new(RichText::new(why).size(12.0).color(color)).wrap());
                    });
                }
            });
            cards.1[i].1 = card_rect(ui, top);
        }
    }
    graph.finish(parent, cards, |from| line(runs[from], &refs[from].title));
    if working {
        parent.ctx().request_repaint_after(SPINNER_FRAME);
    }
}

fn footer(ui: &mut Ui, o: &Orchestrate, step: &Step, team: &[Member], out: &mut Vec<Intent>) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(PAD, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_height(34.0);
                if !o.plan.is_empty() {
                    if o.paused && Button::new("Reanudar", Kind::Secondary).show(ui).clicked() {
                        out.push(Intent::Resume);
                    }
                    if o.plan.iter().all(|(_, run)| run.succeeded())
                        && Button::new("Limpiar worktrees", Kind::Secondary)
                            .show(ui)
                            .clicked()
                    {
                        out.push(Intent::Clean);
                    }
                    if Button::new("Dejar de seguir", Kind::Danger)
                        .icon(Icon::Trash)
                        .show(ui)
                        .on_hover_text(
                            "No lanza las subtareas pendientes. Las pestañas y ramas se conservan. \
                             No se puede deshacer.",
                        )
                        .clicked()
                    {
                        out.push(Intent::Forget);
                    }
                    right(ui, |ui| {
                        if Button::new("Ocultar", Kind::Primary)
                            .keys("Esc")
                            .show(ui)
                            .clicked()
                        {
                            out.push(Intent::Close);
                        }
                    });
                    return;
                }
                match step {
                    Step::Writing => {
                        overlay::keys(ui, "Esc");
                        note(ui, "Cerrar");
                        right(ui, |ui| {
                            let why = plan_blocker(o, team);
                            let button = Button::new("Crear plan", Kind::Primary)
                                .keys(keys!("+Enter"))
                                .enabled(why.is_none())
                                .show(ui);
                            if button.clicked() {
                                out.push(Intent::Plan);
                            }
                            if let Some(why) = why {
                                button.on_hover_text(why);
                                note(ui, why);
                            }
                        });
                    }
                    Step::Planning => right(ui, |ui| {
                        if Button::new("Cancelar", Kind::Secondary).show(ui).clicked() {
                            out.push(Intent::Back);
                        }
                    }),
                    Step::Ready(subtasks, ..) => {
                        if Button::new("Volver", Kind::Ghost)
                            .icon(Icon::ArrowBackUp)
                            .show(ui)
                            .clicked()
                        {
                            out.push(Intent::Back);
                        }
                        right(ui, |ui| {
                            let n = subtasks.len();
                            let text = if n == 1 {
                                "Lanzar 1 agente".into()
                            } else {
                                format!("Lanzar {n} agentes")
                            };
                            if Button::new(&text, Kind::Primary)
                                .keys(keys!("+Enter"))
                                .show(ui)
                                .clicked()
                            {
                                out.push(Intent::Launch);
                            }
                        });
                    }
                    Step::Failed(_) => {
                        if Button::new("Editar la tarea", Kind::Ghost)
                            .icon(Icon::ArrowBackUp)
                            .show(ui)
                            .clicked()
                        {
                            out.push(Intent::Back);
                        }
                        right(ui, |ui| {
                            if Button::new("Reintentar", Kind::Primary)
                                .icon(Icon::Refresh)
                                .show(ui)
                                .clicked()
                            {
                                out.push(Intent::Plan);
                            }
                        });
                    }
                }
            });
        });
}

fn right(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), content);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_read_like_names() {
        assert_eq!(display_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(display_name("claude-fable-5-2"), "Fable 5.2");
        assert_eq!(display_name("deepseek-pro"), "deepseek-pro");
    }

    #[test]
    fn each_subtask_starts_one_round_after_what_it_needs() {
        let task = |after: &[usize]| Subtask {
            title: "t".into(),
            prompt: "p".into(),
            executor: "x".into(),
            after: after.to_vec(),
        };
        let plan = [task(&[]), task(&[]), task(&[0]), task(&[1, 2])];
        let refs: Vec<&Subtask> = plan.iter().collect();
        assert_eq!(rounds(&refs), [1, 1, 2, 3]);
    }

    #[test]
    fn a_graph_line_goes_from_circle_to_circle_through_its_lane() {
        let points = path(pos2(100.0, 10.0), pos2(100.0, 90.0), 40.0);
        assert_eq!(points.first(), Some(&pos2(89.0, 10.0)));
        assert_eq!(points.last(), Some(&pos2(89.0, 90.0)));
        // Llega al carril y nunca se pasa de él, ni vuelve hacia arriba.
        assert!(points.iter().any(|p| p.x == 40.0));
        assert!(points.iter().all(|p| p.x >= 40.0));
        assert!(points.windows(2).all(|w| w[0].y <= w[1].y));
    }

    #[test]
    fn rows_with_only_blanks_or_box_drawing_say_nothing() {
        assert!(!meaningful(""));
        assert!(!meaningful("    "));
        assert!(!meaningful("  ─│┌┐  "));
        assert!(meaningful("│ Leyendo src/app.rs │"));
        assert!(meaningful("cargo test"));
    }

    #[test]
    fn elapsed_time_reads_in_the_biggest_units() {
        let at = |secs| elapsed(Duration::from_secs(secs));
        assert_eq!(at(0), "0 s");
        assert_eq!(at(59), "59 s");
        assert_eq!(at(60), "1 min");
        assert_eq!(at(200), "3 min");
        assert_eq!(at(3600), "1 h");
        assert_eq!(at(3900), "1 h 5 min");
    }

    #[test]
    fn the_active_round_is_the_first_one_that_did_not_succeed() {
        let done = Run::Done {
            branch: "b".into(),
            base: "0".into(),
            code: Some(0),
            commits: true,
        };
        let no_commits = Run::Done {
            branch: "b".into(),
            base: "0".into(),
            code: Some(0),
            commits: false,
        };
        let working = Run::Working {
            pane: 1,
            branch: "b".into(),
            base: "0".into(),
        };
        let blocked = Run::Blocked("x".into());
        let rounds = [1, 1, 2];
        let active = |runs: [&Run; 3]| active_round(&rounds, &runs);
        assert_eq!(active([&done, &done, &done]), None);
        assert_eq!(active([&done, &working, &Run::Waiting]), Some(1));
        assert_eq!(active([&done, &done, &working]), Some(2));
        assert_eq!(active([&done, &blocked, &Run::Waiting]), Some(1));
        assert_eq!(active([&no_commits, &done, &Run::Waiting]), Some(1));
    }
}
