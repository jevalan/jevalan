//! Pantalla del hook: las decisiones de Claude Code, leídas en vivo de `shadow.jsonl`.
//! Cabe señalar que jamás se muestra `tool_input`: las capturas de esta vista acaban en un blog público.

use crate::fx::FxState;
use crate::hook_log::{HookLog, Kind, Summary, bar_geometry, level};
use crate::screen::{Ctx, KeyHint, Msg, Nav, Screen};
use crate::screens::dashboard::footer;
use crate::theme;
use jevalan_hub::shadow::{Entry, NAMES, category};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEventKind};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use std::time::{Duration, Instant};

const KEYS: &[KeyHint] = &[
    KeyHint { key: "↑↓ jk", desc: "elegir decisión" },
    KeyHint { key: "g G", desc: "la más reciente / la más antigua" },
    KeyHint { key: "esc", desc: "volver" },
];

/// Cada cuánto se sondea el fichero.
const POLL_EVERY: Duration = Duration::from_millis(500);
/// Anchura mínima para repartir lista y detalle en horizontal.
const WIDE: u16 = 110;
/// Anchura de una fila de la lista, sin el marcador de selección.
const ROW_WIDTH: u16 = 76;
const LABEL_WIDTH: usize = 19;
const VALUE_WIDTH: usize = 5;

pub struct HookScreen {
    log: Option<HookLog>,
    last_poll: Option<Instant>,
    /// Posición en la lista; 0 es la decisión más reciente.
    selected: usize,
    fx: FxState,
    /// Han llegado datos y falta pintarlos con su destello.
    fresh: bool,
}

impl HookScreen {
    pub fn new(path: Option<std::path::PathBuf>) -> Self {
        Self { log: path.map(HookLog::new), last_poll: None, selected: 0, fx: FxState::new(), fresh: false }
    }

    fn len(&self) -> usize {
        self.log.as_ref().map_or(0, |l| l.entries.len())
    }

    fn poll(&mut self) {
        let Some(log) = &mut self.log else { return };
        let Some(fresh) = log.refresh() else { return };
        if self.selected > 0 {
            // Quien lee una decisión antigua no quiere que se la muevan de debajo.
            self.selected += fresh;
        }
        self.selected = self.selected.min(self.len().saturating_sub(1));
        self.fresh = true;
    }
}

impl Screen for HookScreen {
    fn update(&mut self, _ctx: &mut Ctx, msg: &Msg) -> Option<Nav> {
        match msg {
            Msg::Tick(now) => {
                if self.last_poll.is_none_or(|t| now.saturating_duration_since(t) >= POLL_EVERY) {
                    self.last_poll = Some(*now);
                    self.poll();
                }
            }
            Msg::Key(k) if k.kind == KeyEventKind::Press => {
                let last = self.len().saturating_sub(1);
                match k.code {
                    KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
                    KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
                    KeyCode::Char('G') | KeyCode::End => self.selected = last,
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }

    fn draw(&mut self, ctx: &Ctx, f: &mut Frame, area: Rect) {
        let [head, counters, body, foot] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(4), Constraint::Min(5), Constraint::Length(1)])
                .areas(area);
        let name =
            self.log.as_ref().and_then(|l| l.path().file_name()).map_or("shadow.jsonl".into(), |n| n.to_string_lossy());
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("jevalan", theme::title()),
                Span::styled(format!(" · decisiones del hook · {name}"), theme::text()),
            ])),
            head,
        );

        let empty = Summary::default();
        let summary = self.log.as_ref().map_or(&empty, |l| &l.summary);
        let block = panel("Contadores");
        let inner = block.inner(counters);
        f.render_widget(block, counters);
        f.render_widget(Paragraph::new(counter_lines(summary)), inner);

        let [list_area, detail_area] = if area.width >= WIDE {
            Layout::horizontal([Constraint::Length(ROW_WIDTH + 4), Constraint::Fill(1)]).areas(body)
        } else {
            Layout::vertical([Constraint::Min(4), Constraint::Length(13)]).areas(body)
        };

        let block = panel("Decisiones");
        let list_inner = block.inner(list_area);
        f.render_widget(block, list_area);
        let entries = self.log.as_ref().map(|l| &l.entries);
        match entries.filter(|e| !e.is_empty()) {
            Some(entries) => {
                let items: Vec<ListItem> = entries.iter().rev().map(|e| ListItem::new(row(e))).collect();
                let list = List::new(items)
                    .highlight_symbol("▶ ")
                    .highlight_style(Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED));
                let mut state = ListState::default().with_selected(Some(self.selected));
                f.render_stateful_widget(list, list_inner, &mut state);
            }
            None => {
                let text = self.empty_message();
                let row = Rect::new(
                    list_inner.x,
                    list_inner.y + list_inner.height / 2,
                    list_inner.width,
                    1.min(list_inner.height),
                );
                f.render_widget(Paragraph::new(text).style(theme::dim()).alignment(Alignment::Center), row);
            }
        }

        let selected = self.log.as_ref().and_then(|l| l.entries.iter().rev().nth(self.selected));
        let title = selected.map_or("Detalle".to_string(), |e| format!("Detalle · {} · {}", clock(e), e.tool_name));
        let block = panel(&title);
        let inner = block.inner(detail_area);
        f.render_widget(block, detail_area);
        if let Some(e) = selected {
            render_detail(e, inner, f);
        }

        f.render_widget(Paragraph::new(footer(KEYS)), foot);

        if std::mem::take(&mut self.fresh) {
            self.fx
                .on_arrival(ctx.fx, Rect::new(list_inner.x, list_inner.y, list_inner.width, 1.min(list_inner.height)));
        }
        self.fx.process(ctx.fx, ctx.now, f.buffer_mut(), area);
    }

    fn keys(&self) -> &'static [KeyHint] {
        KEYS
    }

    fn animating(&self) -> bool {
        self.fresh || self.fx.is_running()
    }
}

impl HookScreen {
    fn empty_message(&self) -> String {
        match &self.log {
            None => "no se conoce el directorio de datos · no hay registro que seguir".into(),
            Some(l) if !l.exists => "esperando decisiones… aún no existe shadow.jsonl".into(),
            Some(_) => "el registro está vacío · esperando decisiones…".into(),
        }
    }
}

fn panel(title: &str) -> Block<'static> {
    Block::bordered().border_style(theme::dim()).title(Span::styled(format!(" {title} "), theme::title()))
}

/// Dos líneas: veredictos con su porcentaje y, debajo, latencias y sospechosos.
fn counter_lines(s: &Summary) -> Vec<Line<'static>> {
    let pct = |n: usize| jevalan_hub::shadow::pct(n, s.total);
    let mut top = vec![Span::styled(format!("total {}", s.total), theme::title())];
    for (kind, n) in [
        (Kind::Allow, s.allow),
        (Kind::Ask, s.ask),
        (Kind::Deny, s.deny),
        (Kind::Abstain, s.abstain),
        (Kind::Error, s.error),
    ] {
        top.push(Span::styled("  ", theme::dim()));
        top.push(Span::styled(kind.label(), Style::new().fg(theme::verdict_color(kind)).add_modifier(Modifier::BOLD)));
        top.push(Span::styled(format!(" {n} {:.0}%", pct(n)), theme::text()));
    }
    let ms = |v: Option<u64>| v.map_or("-".to_string(), |v| format!("{v} ms"));
    let mut bottom = vec![Span::styled(format!("p50 {} · p95 {}", ms(s.p50()), ms(s.p95())), theme::text())];
    bottom.push(Span::styled("  ", theme::dim()));
    if s.suspicious > 0 {
        bottom.push(Span::styled(format!(" ask sospechosos {} ", s.suspicious), theme::chip(Kind::Ask)));
    } else {
        bottom.push(Span::styled("ask sospechosos 0", theme::dim()));
    }
    if s.ignored > 0 {
        bottom.push(Span::styled(format!("  líneas ignoradas {}", s.ignored), theme::dim()));
    }
    vec![Line::from(top), Line::from(bottom)]
}

/// `HH:MM:SS` de la marca de tiempo RFC 3339 (UTC).
fn clock(e: &Entry) -> &str {
    e.ts.get(11..19).unwrap_or("--:--:--")
}

/// Recorta a `n` caracteres con puntos suspensivos y rellena hasta `n`.
fn fit(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len <= n { format!("{s:<n$}") } else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}

/// Mayor riesgo con el color de su tramo.
fn risk_span(e: &Entry, width: usize) -> Span<'static> {
    match e.max_risk() {
        Some((name, p)) => Span::styled(
            fit(&format!("{} {p:.2}", fit(name, 15).trim_end()), width),
            Style::new().fg(theme::level_color(level(p))),
        ),
        None => Span::styled(fit("-", width), theme::dim()),
    }
}

fn row(e: &Entry) -> Line<'static> {
    let kind = Kind::of(e);
    let category = if matches!(kind, Kind::Ask | Kind::Deny) {
        // El riesgo dominante ya figura en su columna; aquí basta la clase.
        let full = category(e);
        Span::styled(fit(full.split(':').next().unwrap_or(&full), 17), theme::text())
    } else {
        Span::styled(fit("-", 17), theme::dim())
    };
    let latency = e.latency_ms.map_or("-".to_string(), |ms| format!("{ms}ms"));
    Line::from(vec![
        Span::styled(clock(e).to_string(), theme::dim()),
        Span::raw(" "),
        Span::styled(fit(&e.tool_name, 10), theme::text()),
        Span::raw(" "),
        Span::styled(format!("{:^8}", kind.label()), theme::chip(kind)),
        Span::raw(" "),
        category,
        Span::raw(" "),
        risk_span(e, 20),
        Span::raw(" "),
        Span::styled(format!("{latency:>7}"), theme::dim()),
    ])
}

fn render_detail(e: &Entry, area: Rect, f: &mut Frame) {
    let kind = Kind::of(e);
    let mut meta = vec![
        Span::styled(format!(" {} ", kind.label()), theme::chip(kind)),
        Span::styled(format!("  estado {}", e.status), theme::text()),
    ];
    if let Some(source) = &e.source {
        meta.push(Span::styled(format!(" · fuente {source}"), theme::text()));
    }
    if matches!(kind, Kind::Ask | Kind::Deny) {
        meta.push(Span::styled(format!(" · {}", category(e)), theme::text()));
    }
    let [meta_area, bars, reason] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(NAMES.len() as u16), Constraint::Min(0)])
            .areas(area);
    f.render_widget(Paragraph::new(Line::from(meta)), meta_area);

    let width = (area.width as usize).saturating_sub(LABEL_WIDTH + VALUE_WIDTH + 2);
    let lines: Vec<Line> =
        NAMES.iter().map(|name| bar_line(name, e.probabilities.get(*name).copied(), width)).collect();
    f.render_widget(Paragraph::new(lines), bars);

    let text = e.reason.as_deref().or(e.error.as_deref()).unwrap_or("sin motivo");
    let budget = (reason.width as usize) * (reason.height as usize);
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let text = if text.chars().count() > budget && budget > 0 {
        text.chars().take(budget - 1).collect::<String>() + "…"
    } else {
        text
    };
    f.render_widget(Paragraph::new(text).style(theme::dim()).wrap(Wrap { trim: true }), reason);
}

/// Una barra horizontal con marcas en 0.55 y 0.85; `authorized` no es un riesgo y va aparte, sin marcas.
fn bar_line(name: &str, p: Option<f64>, width: usize) -> Line<'static> {
    let authorized = name == "authorized";
    let label_style = if authorized { theme::accent().add_modifier(Modifier::ITALIC) } else { theme::text() };
    let mut spans = vec![Span::styled(fit(name, LABEL_WIDTH), label_style), Span::raw(" ")];
    let (filled, ticks) = p.map_or((0, [width, width]), |p| bar_geometry(p, width));
    let fill = match p {
        _ if authorized => theme::ACCENT,
        Some(p) => theme::level_color(level(p)),
        None => theme::IDLE,
    };
    for i in 0..width {
        let tick = !authorized && p.is_some() && ticks.contains(&i);
        let span = match (i < filled, tick) {
            (true, true) => Span::styled("┃", Style::new().fg(theme::BACKDROP).bg(fill)),
            (true, false) => Span::styled("█", Style::new().fg(fill)),
            (false, true) => Span::styled("│", theme::text()),
            (false, false) => Span::styled("░", theme::dim()),
        };
        spans.push(span);
    }
    let value = p.map_or("   -".to_string(), |p| format!(" {p:.2}"));
    spans.push(Span::styled(value, label_style));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxMode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("jevalan-hook-screen-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn render(screen: &mut HookScreen, ctx: &Ctx, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| screen.draw(ctx, f, f.area())).unwrap();
        term.backend().to_string()
    }

    const LOG: &str = concat!(
        r#"{"ts":"2026-01-01T10:00:01Z","tool_name":"Bash","tool_input":{"command":"TOPSECRET rm -rf"},"status":"ok","verdict":"allow","source":"model","probabilities":{"destructive":0.1,"authorized":0.9},"latency_ms":120}"#,
        "\n",
        r#"{"ts":"2026-01-01T10:00:02Z","tool_name":"Write","tool_input":{"content":"TOPSECRET"},"status":"ok","verdict":"ask","source":"model","reason":"duda razonable","probabilities":{"destructive":0.3,"secret_exposure":0.7,"authorized":0.2},"latency_ms":340}"#,
        "\n",
        r#"{"ts":"2026-01-01T10:00:03Z","tool_name":"Edit","tool_input":"TOPSECRET","status":"error","verdict":null,"error":"daemon inalcanzable","latency_ms":5}"#,
        "\n",
    );

    fn tick(screen: &mut HookScreen, ctx: &mut Ctx, now: Instant) {
        ctx.now = now;
        screen.update(ctx, &Msg::Tick(now));
    }

    #[test]
    fn empty_state_when_the_log_does_not_exist() {
        let mut ctx = Ctx::new(FxMode::Events, Instant::now());
        let mut screen = HookScreen::new(Some(temp("absent")));
        let now = ctx.now;
        tick(&mut screen, &mut ctx, now);
        let out = render(&mut screen, &ctx, 80, 24);
        assert!(out.contains("esperando decisiones"), "{out}");
        assert!(out.contains("total 0"));
        let mut nowhere = HookScreen::new(None);
        assert!(render(&mut nowhere, &ctx, 80, 24).contains("no se conoce el directorio"));
    }

    #[test]
    fn renders_decisions_and_never_the_tool_input() {
        let path = temp("render");
        std::fs::write(&path, LOG).unwrap();
        let t0 = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, t0);
        let mut screen = HookScreen::new(Some(path.clone()));
        tick(&mut screen, &mut ctx, t0);
        for (w, h) in [(80, 24), (120, 30)] {
            let out = render(&mut screen, &ctx, w, h);
            assert!(!out.contains("TOPSECRET"), "fuga de tool_input en\n{out}");
            for want in ["total 3", "Bash", "Write", "Edit", "allow", "ask", "error", "secret_exposure"] {
                assert!(out.contains(want), "falta {want:?} en\n{out}");
            }
        }
        // La seleccionada por defecto es la más reciente: el error, con su texto.
        assert!(render(&mut screen, &ctx, 120, 30).contains("daemon inalcanzable"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn selection_follows_keys_and_stays_put_when_data_arrives() {
        let path = temp("select");
        std::fs::write(&path, LOG).unwrap();
        let t0 = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, t0);
        let mut screen = HookScreen::new(Some(path.clone()));
        tick(&mut screen, &mut ctx, t0);
        assert!(screen.animating(), "los datos nuevos piden un repintado");
        render(&mut screen, &ctx, 80, 24);
        let press = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));
        screen.update(&mut ctx, &press(KeyCode::Down));
        assert_eq!(screen.selected, 1);
        screen.update(&mut ctx, &press(KeyCode::Char('G')));
        assert_eq!(screen.selected, 2);
        screen.update(&mut ctx, &press(KeyCode::Down));
        assert_eq!(screen.selected, 2);
        std::fs::write(&path, format!("{LOG}{LOG}")).unwrap();
        tick(&mut screen, &mut ctx, t0 + Duration::from_millis(600));
        assert_eq!(screen.selected, 5, "sigue sobre la misma decisión");
        screen.update(&mut ctx, &press(KeyCode::Char('g')));
        assert_eq!(screen.selected, 0);
        let _ = std::fs::remove_file(&path);
    }
}
