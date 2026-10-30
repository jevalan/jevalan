//! Hook screen: Claude Code's decisions, read live from `shadow.jsonl`.
//! Note that `tool_input` is never shown: screenshots of this view end up on a public blog.

use crate::fx::FxState;
use crate::hook_log::{HookLog, Kind, Summary, bar_geometry, level};
use crate::screen::{Ctx, KeyHint, Msg, Nav, Screen};
use crate::screens::dashboard::footer;
use crate::theme;
use jevalan_hub::shadow::{Entry, NAMES, category, suspicious};
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
    KeyHint { key: "1-5", desc: "solo allow/ask/deny/abstain/error (0 quita)" },
    KeyHint { key: "s", desc: "solo ask sospechosos" },
    KeyHint { key: "/", desc: "filtrar por herramienta (enter fija, esc borra)" },
    KeyHint { key: "c", desc: "quitar los filtros" },
    KeyHint { key: "esc", desc: "volver" },
];

/// How often the file is probed once it is loaded.
const POLL_EVERY: Duration = Duration::from_millis(500);
/// Bytes of the log taken per tick while it loads, so that the screen keeps answering and can show its progress.
const LOAD_CHUNK: u64 = 1 << 19;
/// Longest tool filter that can be typed.
const MAX_TOOL_FILTER: usize = 24;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// The verdicts that keys `1` to `5` select.
const FILTER_KINDS: [Kind; 5] = [Kind::Allow, Kind::Ask, Kind::Deny, Kind::Abstain, Kind::Error];
/// Minimum width to split list and detail horizontally.
const WIDE: u16 = 110;
/// Width of a list row, without the selection marker.
const ROW_WIDTH: u16 = 76;
const LABEL_WIDTH: usize = 19;
const VALUE_WIDTH: usize = 5;

/// What narrows the list of decisions. The counters above it always speak for the whole log.
#[derive(Debug, Default, Clone, PartialEq)]
struct Filter {
    kind: Option<Kind>,
    suspicious: bool,
    /// Part of the tool's name, any case.
    tool: String,
}

impl Filter {
    fn active(&self) -> bool {
        self.kind.is_some() || self.suspicious || !self.tool.is_empty()
    }

    fn passes(&self, e: &Entry) -> bool {
        self.kind.is_none_or(|k| Kind::of(e) == k)
            && (!self.suspicious || suspicious(e))
            && (self.tool.is_empty() || e.tool_name.to_lowercase().contains(&self.tool.to_lowercase()))
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(kind) = self.kind {
            parts.push(kind.label().to_string());
        }
        if self.suspicious {
            parts.push("sospechosos".into());
        }
        if !self.tool.is_empty() {
            parts.push(format!("/{}", self.tool));
        }
        parts.join(" · ")
    }
}

pub struct HookScreen {
    log: Option<HookLog>,
    last_poll: Option<Instant>,
    /// When the first probe happened, and how long the whole file took to load once it did.
    started: Option<Instant>,
    loaded_in: Option<Duration>,
    /// Bytes taken per tick while loading.
    chunk: u64,
    /// Position in the filtered list; 0 is the most recent decision.
    selected: usize,
    filter: Filter,
    /// The tool filter is being typed.
    editing: bool,
    fx: FxState,
    /// Data has arrived and still needs painting with its flash.
    fresh: bool,
}

impl HookScreen {
    pub fn new(path: Option<std::path::PathBuf>) -> Self {
        Self {
            log: path.map(HookLog::new),
            last_poll: None,
            started: None,
            loaded_in: None,
            chunk: LOAD_CHUNK,
            selected: 0,
            filter: Filter::default(),
            editing: false,
            fx: FxState::new(),
            fresh: false,
        }
    }

    /// The decisions that pass the filter, most recent first.
    fn visible(&self) -> Vec<&Entry> {
        self.log.iter().flat_map(|l| l.entries.iter().rev()).filter(|e| self.filter.passes(e)).collect()
    }

    fn len(&self) -> usize {
        self.log.as_ref().map_or(0, |l| l.entries.iter().filter(|e| self.filter.passes(e)).count())
    }

    /// The log has not been read through yet (or not even probed).
    fn loading(&self) -> bool {
        self.log.as_ref().is_some_and(|l| self.started.is_none() || l.loading())
    }

    fn poll(&mut self, now: Instant) {
        let Some(log) = &mut self.log else { return };
        let started = *self.started.get_or_insert(now);
        let fresh = log.refresh_up_to(self.chunk);
        if !log.loading() && self.loaded_in.is_none() {
            self.loaded_in = Some(now.saturating_duration_since(started));
        }
        let Some(fresh) = fresh else { return };
        if self.selected > 0 {
            // Whoever is reading an old decision does not want it moved from under them.
            self.selected += log.entries.iter().rev().take(fresh).filter(|e| self.filter.passes(e)).count();
        }
        let shown = log.entries.iter().filter(|e| self.filter.passes(e)).count();
        self.selected = self.selected.min(shown.saturating_sub(1));
        self.fresh = true;
    }

    fn change_filter(&mut self, filter: Filter) {
        if filter != self.filter {
            self.filter = filter;
            self.selected = 0;
        }
    }

    fn navigate(&mut self, code: KeyCode) {
        let last = self.len().saturating_sub(1);
        let mut filter = self.filter.clone();
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.selected = last,
            KeyCode::Char('0') => filter.kind = None,
            KeyCode::Char(c @ '1'..='5') => filter.kind = FILTER_KINDS.get(c as usize - '1' as usize).copied(),
            KeyCode::Char('s') => filter.suspicious = !filter.suspicious,
            KeyCode::Char('c') => filter = Filter::default(),
            KeyCode::Char('/') => self.editing = true,
            _ => {}
        }
        self.change_filter(filter);
    }

    fn type_filter(&mut self, code: KeyCode) {
        let mut filter = self.filter.clone();
        match code {
            KeyCode::Enter => self.editing = false,
            KeyCode::Esc => {
                filter.tool.clear();
                self.editing = false;
            }
            KeyCode::Backspace => {
                filter.tool.pop();
            }
            KeyCode::Char(c) if !c.is_control() && filter.tool.chars().count() < MAX_TOOL_FILTER => filter.tool.push(c),
            _ => {}
        }
        self.change_filter(filter);
    }
}

impl Screen for HookScreen {
    fn update(&mut self, _ctx: &mut Ctx, msg: &Msg) -> Option<Nav> {
        match msg {
            Msg::Tick(now) => {
                // While loading every tick takes one more slice; afterwards the file is probed twice a second.
                if self.loading() || self.last_poll.is_none_or(|t| now.saturating_duration_since(t) >= POLL_EVERY) {
                    self.last_poll = Some(*now);
                    self.poll(*now);
                }
            }
            Msg::Key(k) if k.kind == KeyEventKind::Press => {
                if self.editing {
                    self.type_filter(k.code);
                } else {
                    self.navigate(k.code);
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
        let mut title = vec![
            Span::styled("jevalan", theme::title()),
            Span::styled(format!(" · decisiones del hook · {name}"), theme::text()),
        ];
        if let (Some(log), true) = (&self.log, self.loading()) {
            let (read, total) = log.progress();
            title.push(Span::styled(format!(" · cargando {}%", percent(read, total)), theme::dim()));
        }
        f.render_widget(Paragraph::new(Line::from(title)), head);

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

        let retained = self.log.as_ref().map_or(0, |l| l.entries.len());
        let (items, selected): (Vec<ListItem>, Option<Entry>) = {
            let visible = self.visible();
            (visible.iter().map(|e| ListItem::new(row(e))).collect(), visible.get(self.selected).map(|e| (*e).clone()))
        };
        let mut list_title = format!("Decisiones · {} de {retained}", items.len());
        if self.filter.active() || self.editing {
            list_title.push_str(&format!(" · filtro: {}", self.filter.describe()));
        }
        if self.editing {
            list_title.push('▏');
        }
        let block = panel(&list_title);
        let list_inner = block.inner(list_area);
        f.render_widget(block, list_area);
        if self.loading() {
            self.draw_loading(ctx, f, list_inner);
        } else if items.is_empty() {
            let row = Rect::new(
                list_inner.x,
                list_inner.y + list_inner.height / 2,
                list_inner.width,
                1.min(list_inner.height),
            );
            f.render_widget(Paragraph::new(self.empty_message()).style(theme::dim()).alignment(Alignment::Center), row);
        } else {
            let list = List::new(items)
                .highlight_symbol("▶ ")
                .highlight_style(Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED));
            let mut state = ListState::default().with_selected(Some(self.selected));
            f.render_stateful_widget(list, list_inner, &mut state);
        }

        let title =
            selected.as_ref().map_or("Detalle".to_string(), |e| format!("Detalle · {} · {}", clock(e), e.tool_name));
        let block = panel(&title);
        let inner = block.inner(detail_area);
        f.render_widget(block, detail_area);
        if let Some(e) = &selected {
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
        self.fresh || self.fx.is_running() || self.loading()
    }

    fn captures_text(&self) -> bool {
        self.editing
    }
}

impl HookScreen {
    fn empty_message(&self) -> String {
        match &self.log {
            None => "no se conoce el directorio de datos · no hay registro que seguir".into(),
            Some(l) if !l.exists => "esperando decisiones… aún no existe shadow.jsonl".into(),
            Some(l) if l.entries.is_empty() => "el registro está vacío · esperando decisiones…".into(),
            Some(_) => "ninguna decisión coincide con el filtro · c lo quita".into(),
        }
    }

    /// A spinner, a bar and the counts of what has been read, centred in the list.
    fn draw_loading(&self, ctx: &Ctx, f: &mut Frame, area: Rect) {
        let Some(log) = &self.log else { return };
        let (read, total) = log.progress();
        let elapsed = self.started.map_or(Duration::ZERO, |t| ctx.now.saturating_duration_since(t));
        let spinner = SPINNER[(elapsed.as_millis() / 80) as usize % SPINNER.len()];
        let width = (area.width as usize).saturating_sub(8).clamp(10, 40);
        let filled = if total == 0 { 0 } else { (read as f64 / total as f64 * width as f64).round() as usize };
        let bar: String = "█".repeat(filled.min(width)) + &"░".repeat(width - filled.min(width));
        let lines = vec![
            Line::from(Span::styled(format!("{spinner} Cargando el registro…"), theme::title())),
            Line::from(vec![
                Span::styled(bar, theme::accent()),
                Span::styled(format!(" {:>3}%", percent(read, total)), theme::text()),
            ]),
            Line::from(Span::styled(
                format!(
                    "{} de {} MB · {} decisiones · {:.1} s",
                    mb(read),
                    mb(total),
                    log.summary.total,
                    elapsed.as_secs_f64()
                ),
                theme::dim(),
            )),
        ];
        let height = (lines.len() as u16).min(area.height);
        let row = Rect::new(area.x, area.y + area.height.saturating_sub(height) / 2, area.width, height);
        f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), row);
    }
}

/// Percentage of `read` over `total`, whole; an empty file counts as done.
fn percent(read: u64, total: u64) -> u64 {
    if total == 0 { 100 } else { (read * 100 / total).min(100) }
}

/// Mebibytes with one decimal and a comma.
fn mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_048_576.0).replace('.', ",")
}

fn panel(title: &str) -> Block<'static> {
    Block::bordered().border_style(theme::dim()).title(Span::styled(format!(" {title} "), theme::title()))
}

/// Two lines: verdicts with their percentage and, below, latencies and suspicious ones.
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

/// `HH:MM:SS` of the RFC 3339 timestamp (UTC).
fn clock(e: &Entry) -> &str {
    e.ts.get(11..19).unwrap_or("--:--:--")
}

/// Trims to `n` characters with an ellipsis and pads up to `n`.
fn fit(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len <= n { format!("{s:<n$}") } else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}

/// Highest risk with the colour of its band.
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
        // The dominant risk already appears in its column; the class is enough here.
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

/// A horizontal bar with marks at 0.55 and 0.85; `authorized` is not a risk and goes apart, without marks.
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
        // The one selected by default is the most recent: the error, with its text.
        assert!(render(&mut screen, &ctx, 120, 30).contains("daemon inalcanzable"));
        let _ = std::fs::remove_file(&path);
    }

    /// Ticks until the log is read through, at most a hundred.
    fn load(screen: &mut HookScreen, ctx: &mut Ctx, t0: Instant) -> usize {
        for n in 0..100 {
            if !screen.loading() && n > 0 {
                return n;
            }
            tick(screen, ctx, t0 + Duration::from_millis(16 * n as u64));
        }
        100
    }

    #[test]
    fn a_large_log_loads_in_slices_and_says_how_far_it_got() {
        let path = temp("slices");
        std::fs::write(&path, LOG.repeat(4)).unwrap();
        let t0 = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, t0);
        let mut screen = HookScreen::new(Some(path.clone()));
        screen.chunk = 300;
        assert!(screen.loading(), "antes de la primera lectura ya se está cargando");

        tick(&mut screen, &mut ctx, t0);
        assert!(screen.loading() && screen.animating());
        let out = render(&mut screen, &ctx, 100, 24);
        for want in ["Cargando el registro", "cargando", "MB", "decisiones", "%"] {
            assert!(out.contains(want), "falta {want:?} en\n{out}");
        }
        assert!(!out.contains("TOPSECRET"));

        let ticks = load(&mut screen, &mut ctx, t0);
        assert!(ticks > 2 && ticks < 100, "se esperaban varias rebanadas, no {ticks}");
        assert!(!screen.loading() && screen.loaded_in.is_some());
        assert!(render(&mut screen, &ctx, 100, 24).contains("total 12"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_verdict_keys_narrow_the_list_and_the_counters_stay_whole() {
        let path = temp("kind");
        std::fs::write(&path, LOG).unwrap();
        let t0 = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, t0);
        let mut screen = HookScreen::new(Some(path.clone()));
        load(&mut screen, &mut ctx, t0);
        let press = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));

        screen.update(&mut ctx, &press(KeyCode::Char('2')));
        assert_eq!(screen.len(), 1);
        let out = render(&mut screen, &ctx, 120, 30);
        assert!(out.contains("1 de 3") && out.contains("filtro: ask"), "{out}");
        assert!(out.contains("total 3"), "los contadores hablan de todo el registro");
        assert!(out.contains("duda razonable"), "el detalle es el de la decisión filtrada");

        screen.update(&mut ctx, &press(KeyCode::Char('3')));
        assert!(render(&mut screen, &ctx, 120, 30).contains("ninguna decisión coincide"));

        screen.update(&mut ctx, &press(KeyCode::Char('c')));
        assert_eq!(screen.len(), 3);
        screen.update(&mut ctx, &press(KeyCode::Char('s')));
        assert!(screen.filter.suspicious);
        screen.update(&mut ctx, &press(KeyCode::Char('0')));
        screen.update(&mut ctx, &press(KeyCode::Char('c')));
        assert!(!screen.filter.active());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_tool_filter_is_typed_without_the_global_keys_and_esc_clears_it() {
        let path = temp("tool");
        std::fs::write(&path, LOG).unwrap();
        let t0 = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, t0);
        let mut screen = HookScreen::new(Some(path.clone()));
        load(&mut screen, &mut ctx, t0);
        let press = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));

        assert!(!screen.captures_text());
        screen.update(&mut ctx, &press(KeyCode::Char('/')));
        assert!(screen.captures_text(), "mientras se escribe, q, f y esc son del filtro");
        for c in "EDI".chars() {
            screen.update(&mut ctx, &press(KeyCode::Char(c)));
        }
        assert_eq!((screen.len(), screen.filter.tool.as_str()), (1, "EDI"), "sin distinguir mayúsculas");
        assert!(render(&mut screen, &ctx, 120, 30).contains("/EDI▏"));
        screen.update(&mut ctx, &press(KeyCode::Backspace));
        assert_eq!(screen.filter.tool, "ED");

        screen.update(&mut ctx, &press(KeyCode::Enter));
        assert!(!screen.captures_text() && screen.filter.tool == "ED", "enter fija el filtro");
        screen.update(&mut ctx, &press(KeyCode::Char('/')));
        screen.update(&mut ctx, &press(KeyCode::Esc));
        assert!(!screen.captures_text() && screen.filter.tool.is_empty(), "esc lo borra");
        assert_eq!(screen.len(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn data_arriving_does_not_move_the_decision_being_read_under_a_filter() {
        let path = temp("filtered-arrival");
        std::fs::write(&path, LOG).unwrap();
        let t0 = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, t0);
        let mut screen = HookScreen::new(Some(path.clone()));
        load(&mut screen, &mut ctx, t0);
        let press = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));
        screen.update(&mut ctx, &press(KeyCode::Char('1')));
        assert_eq!(screen.len(), 1);
        // Only `Bash` is an allow; no more arrive, so the selection has nothing to shift for.
        std::fs::write(&path, format!("{LOG}{}", LOG.lines().nth(1).unwrap().to_string() + "\n")).unwrap();
        tick(&mut screen, &mut ctx, t0 + Duration::from_secs(2));
        assert_eq!((screen.len(), screen.selected), (1, 0));
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
