//! Pantalla principal: topología, métricas y feed de decisiones.

use crate::fx::FxState;
use crate::screen::{Ctx, KeyHint, Msg, Nav, Screen, SourceStatus};
use crate::screens::capability::CapabilityScreen;
use crate::screens::flow::FlowScreen;
use crate::screens::hook::HookScreen;
use crate::screens::trace::TraceScreen;
use crate::theme;
use crate::topology::{NodeId, PASSTHROUGH};
use crate::widgets::topology::TopologyView;
use crate::widgets::{feed, metrics};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEventKind};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

const KEYS: &[KeyHint] = &[
    KeyHint { key: "tab", desc: "foco topología / feed" },
    KeyHint { key: "enter", desc: "abrir flujo o capacidad (puerto)" },
    KeyHint { key: "←↑↓→ hjkl", desc: "mover la vista (topología)" },
    KeyHint { key: "+ -", desc: "zoom" },
    KeyHint { key: "0", desc: "ajustar a la vista" },
    KeyHint { key: "n N", desc: "nodo siguiente / anterior" },
    KeyHint { key: "↑↓ enter", desc: "elegir decisión y abrir su traza (feed)" },
    KeyHint { key: "H", desc: "decisiones del hook de Claude Code" },
];

const KEY_TOGGLE: KeyHint = KeyHint { key: "x", desc: "apagar / encender backend" };
const NOTICE_TTL: std::time::Duration = std::time::Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Topology,
    Feed,
}

pub struct Dashboard {
    pub focus: Focus,
    pub feed_selected: usize,
    topo_view: TopologyView,
    topo_area: Rect,
    fx: FxState,
    started: bool,
}

impl Dashboard {
    pub fn new() -> Self {
        Self {
            focus: Focus::Topology,
            feed_selected: 0,
            topo_view: TopologyView::new(),
            topo_area: Rect::default(),
            fx: FxState::new(),
            started: false,
        }
    }
}

impl Default for Dashboard {
    fn default() -> Self {
        Self::new()
    }
}

/// Cabecera: fuente, ritmo, modo de efectos y avisos.
pub fn header(ctx: &Ctx) -> Line<'static> {
    let link = match &ctx.source_status {
        None => String::new(),
        Some(SourceStatus::Connecting) => " · conectando…".into(),
        Some(SourceStatus::Connected) => " · conectado".into(),
        Some(SourceStatus::Disconnected(why)) => format!(" · sin conexión: {why} (reintentando)"),
    };
    let mut spans = vec![
        Span::styled("jevalan", theme::title()),
        Span::styled(
            format!(" · {}{link} · {:.1} req/s · fx:{}", ctx.source, ctx.store.rate(ctx.now), ctx.fx),
            theme::text(),
        ),
    ];
    if let Some((text, born)) = &ctx.notice
        && ctx.now.saturating_duration_since(*born) < NOTICE_TTL
    {
        spans.push(Span::styled(format!(" · {text}"), theme::accent()));
    }
    if ctx.paused {
        spans.push(Span::styled(" · PAUSA", theme::error()));
    }
    if ctx.source_closed {
        spans.push(Span::styled(" · fuente: cerrada", theme::error()));
    }
    if ctx.store.dropped() > 0 {
        spans.push(Span::styled(format!(" · descartados: {}", ctx.store.dropped()), theme::error()));
    }
    Line::from(spans)
}

/// Pie: atajos de la pantalla y los globales imprescindibles.
pub fn footer(keys: &[KeyHint]) -> Line<'static> {
    let mut spans = Vec::new();
    for k in keys.iter().chain(&[KeyHint { key: "?", desc: "ayuda" }, KeyHint { key: "q", desc: "salir" }]) {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", theme::dim()));
        }
        spans.push(Span::styled(k.key, theme::accent()));
        spans.push(Span::styled(format!(" {}", k.desc), theme::dim()));
    }
    Line::from(spans)
}

/// Reparto vertical: cabecera, topología, métricas, feed, pie.
pub fn areas(area: Rect) -> [Rect; 5] {
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(3),
        Constraint::Length(7),
        Constraint::Length(1),
    ])
    .areas(area)
}

fn panel(title: &str, focused: bool) -> Block<'static> {
    let style = if focused { theme::accent() } else { theme::dim() };
    Block::bordered().border_style(style).title(Span::styled(format!(" {title} "), theme::title()))
}

impl Screen for Dashboard {
    fn update(&mut self, ctx: &mut Ctx, msg: &Msg) -> Option<Nav> {
        match msg {
            Msg::Key(k) if k.kind == KeyEventKind::Press && k.code == KeyCode::Char('H') => {
                return Some(Nav::Push(Box::new(HookScreen::new(ctx.shadow_log.clone()))));
            }
            Msg::Key(k) if k.kind == KeyEventKind::Press && k.code == KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Topology => Focus::Feed,
                    Focus::Feed => Focus::Topology,
                };
            }
            Msg::Key(k) if k.kind == KeyEventKind::Press && self.focus == Focus::Feed => {
                let decision_count = ctx.store.decisions().count();
                match k.code {
                    KeyCode::Up => {
                        if decision_count > 0 {
                            self.feed_selected = self.feed_selected.saturating_sub(1);
                        }
                    }
                    KeyCode::Down => {
                        if decision_count > 0 {
                            let max_idx = decision_count.saturating_sub(1);
                            if self.feed_selected < max_idx {
                                self.feed_selected += 1;
                            }
                        }
                    }
                    KeyCode::Enter => {
                        let decisions: Vec<_> = ctx.store.decisions().collect();
                        if decision_count > 0
                            && self.feed_selected < decisions.len()
                            && let Some(decision) = decisions.get(self.feed_selected)
                        {
                            return Some(Nav::Push(Box::new(TraceScreen::new(decision.trace_id.clone()))));
                        }
                    }
                    _ => {}
                }
            }
            Msg::Key(k)
                if k.kind == KeyEventKind::Press && k.code == KeyCode::Enter && self.focus == Focus::Topology =>
            {
                if let Some(NodeId::Port(name)) = self.topo_view.selected()
                    && name != PASSTHROUGH
                {
                    // Una capacidad servida por un flujo abre el flujo; la de un especialista, su ficha.
                    let screen: Box<dyn Screen> = match ctx.topo.capability(name) {
                        Some(cap) => match (cap.kind.as_str(), &cap.flow) {
                            ("flow", Some(flow)) => Box::new(FlowScreen::new(flow.clone())),
                            _ => Box::new(CapabilityScreen::new(name.clone())),
                        },
                        None => Box::new(FlowScreen::new(name.clone())),
                    };
                    return Some(Nav::Push(screen));
                }
            }
            Msg::Key(k)
                if k.kind == KeyEventKind::Press && k.code == KeyCode::Char('x') && self.focus == Focus::Topology =>
            {
                if let (Some(NodeId::Backend(name)), Some(toggle)) = (self.topo_view.selected(), &ctx.toggle)
                    && let Some(down) = toggle(name)
                {
                    let text =
                        if down { format!("{name} apagado (x para encender)") } else { format!("{name} encendido") };
                    ctx.notice = Some((text, ctx.now));
                }
            }
            Msg::Key(_) if self.focus == Focus::Topology => {
                self.topo_view.handle(msg, self.topo_area);
            }
            Msg::Mouse(m) if self.topo_area.contains((m.column, m.row).into()) => {
                self.topo_view.handle(msg, self.topo_area);
            }
            Msg::Tick(_) => {
                for cue in &ctx.cues {
                    self.fx.on_cue(cue, ctx.fx, |id| self.topo_view.node_area(id));
                }
            }
            _ => {}
        }
        None
    }

    fn draw(&mut self, ctx: &Ctx, f: &mut Frame, area: Rect) {
        let [head, topo, metrics_area, feed_area, foot] = areas(area);
        f.render_widget(Paragraph::new(header(ctx)), head);
        let block = panel("Topología", self.focus == Focus::Topology);
        let inner = block.inner(topo);
        f.render_widget(block, topo);
        self.topo_area = inner;
        self.topo_view.render(&ctx.topo, ctx.now, ctx.fx, inner, f.buffer_mut());
        self.topo_view.render_latency(&ctx.store, ctx.now, inner, f.buffer_mut());
        if !ctx.topo.has_live_packets() && ctx.topo.visible_clients().is_empty() {
            let row = Rect::new(inner.x, inner.y + inner.height / 2, inner.width, 1.min(inner.height));
            f.render_widget(
                Paragraph::new("esperando consultas…").style(theme::dim()).alignment(Alignment::Center),
                row,
            );
        }

        // Render metrics panel.
        let block = panel("Métricas", false);
        let inner = block.inner(metrics_area);
        f.render_widget(block, metrics_area);
        metrics::render(ctx, self.topo_view.selected(), inner, f.buffer_mut());

        // Render feed panel.
        let block = panel("Decisiones", self.focus == Focus::Feed);
        let inner = block.inner(feed_area);
        f.render_widget(block, feed_area);
        feed::render(ctx, self.feed_selected, self.focus == Focus::Feed, inner, f.buffer_mut());

        // Footer: tab key + keys for the focused panel.
        let panel_keys = match self.focus {
            Focus::Topology => &KEYS[1..6],
            Focus::Feed => &KEYS[6..7],
        };
        let mut footer_keys = vec![KEYS[0], KEYS[7]];
        if ctx.toggle.is_some() && self.focus == Focus::Topology {
            footer_keys.push(KEY_TOGGLE);
        }
        footer_keys.extend_from_slice(panel_keys);
        f.render_widget(Paragraph::new(footer(&footer_keys)), foot);

        if !self.started {
            self.started = true;
            self.fx.on_start(ctx.fx, area);
        }
        self.fx.sync_ambient(ctx.fx, self.topo_area);
        self.fx.process(ctx.fx, ctx.now, f.buffer_mut(), area);
    }

    fn animating(&self) -> bool {
        self.fx.is_running() || self.topo_view.is_moving()
    }

    fn keys(&self) -> &'static [KeyHint] {
        KEYS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxMode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
    use std::time::Instant;

    /// Dashboard sin la transición de arranque, para que las capturas no dependan del reloj.
    fn settled() -> Dashboard {
        Dashboard { started: true, ..Dashboard::new() }
    }

    fn render(d: &mut Dashboard, ctx: &Ctx) -> String {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| d.draw(ctx, f, f.area())).unwrap();
        term.backend().to_string()
    }

    fn decision_event(trace: &str, outcome: &str) -> jevalan_events::Event {
        let kind = jevalan_events::EventKind::DecisionMade {
            flow: "agent-guard".into(),
            outcome: outcome.into(),
            path_len: 1,
            duration_ms: 5,
        };
        jevalan_events::new_event(trace, 1, "test", kind)
    }

    #[test]
    fn header_shows_source_rate_and_warnings() {
        let mut ctx = Ctx::new(FxMode::Full, Instant::now());
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(header(&ctx)), "jevalan · fake · 0.0 req/s · fx:full");
        ctx.paused = true;
        ctx.source_closed = true;
        ctx.store.note_dropped();
        assert_eq!(
            text(header(&ctx)),
            "jevalan · fake · 0.0 req/s · fx:full · PAUSA · fuente: cerrada · descartados: 1"
        );
    }

    #[test]
    fn tab_toggles_focus() {
        let mut d = Dashboard::new();
        let mut ctx = Ctx::new(FxMode::Events, Instant::now());
        d.update(&mut ctx, &Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        assert_eq!(d.focus, Focus::Feed);
        d.update(&mut ctx, &Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        assert_eq!(d.focus, Focus::Topology);
    }

    #[test]
    fn recovers_after_the_terminal_grows_back() {
        let mut app = crate::app::App::new(Box::new(settled()), FxMode::Events, Instant::now());
        let mut small = Terminal::new(TestBackend::new(40, 10)).unwrap();
        small.draw(|f| app.draw(f)).unwrap();
        assert!(small.backend().to_string().contains("terminal demasiado pequeño"));
        let mut big = Terminal::new(TestBackend::new(80, 24)).unwrap();
        big.draw(|f| app.draw(f)).unwrap();
        assert!(big.backend().to_string().contains("jevalan · fake"));
    }

    #[test]
    fn skeleton_snapshot() {
        let ctx = Ctx::new(FxMode::Events, Instant::now());
        insta::assert_snapshot!(render(&mut settled(), &ctx));
    }

    #[test]
    fn enter_on_an_empty_feed_does_nothing() {
        let mut d = Dashboard::new();
        let mut ctx = Ctx::new(FxMode::Events, Instant::now());
        d.update(&mut ctx, &Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        assert!(d.update(&mut ctx, &Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))).is_none());
        d.update(&mut ctx, &Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        assert_eq!(d.feed_selected, 0);
    }

    #[test]
    fn feed_selection_is_clamped() {
        let mut d = Dashboard::new();
        let now = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, now);
        for i in 0..2 {
            ctx.store.ingest(decision_event(&format!("t{i}"), "allow"), now);
        }
        d.update(&mut ctx, &Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        for _ in 0..5 {
            d.update(&mut ctx, &Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        }
        assert_eq!(d.feed_selected, 1);
        d.update(&mut ctx, &Msg::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
        assert_eq!(d.feed_selected, 0);
    }

    /// Contexto con los flujos y las capacidades de ejemplo cargados y algo de tráfico: doce decisiones por flujo
    /// y una docena de peticiones al hub (para el flujo `agent-wakeup` y para el especialista `toolgate`).
    fn hub_ctx(now: Instant) -> Ctx {
        let mut ctx = Ctx::new(FxMode::Events, now);
        let (flows, wakeup) = (crate::fake::flows(), crate::fake::wakeup_flow());
        let announced = crate::fake::loaded_events(&flows)
            .into_iter()
            .chain(crate::fake::loaded_events(std::slice::from_ref(&wakeup)))
            .chain(crate::fake::capability_events());
        for e in announced {
            ctx.topo.reduce(&e, now);
            ctx.store.ingest(e, now);
        }
        let mut rng = crate::fake::Rng::new(3);
        for i in 0..12 {
            let traces = [
                crate::fake::trace(&mut rng, &format!("t{i}"), "crush", &flows, None),
                crate::fake::hub_trace(&mut rng, &format!("h{i}"), ["vscode", "cron"][i % 2], &wakeup, None),
            ];
            for (_, e) in traces.into_iter().flatten() {
                ctx.topo.reduce(&e, now);
                ctx.store.ingest(e, now);
            }
        }
        ctx.topo.tick(now + crate::topology::PACKET_TTL * 2);
        ctx
    }

    #[test]
    fn with_traffic_shows_metrics_and_feed() {
        let mut d = settled();
        let ctx = hub_ctx(Instant::now());
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| d.draw(&ctx, f, f.area())).unwrap();
        insta::assert_snapshot!(term.backend().to_string());
    }

    #[test]
    fn hub_responses_show_up_in_the_feed_and_the_metrics() {
        let mut d = settled();
        let ctx = hub_ctx(Instant::now());
        let out = render_wide(&mut d, &ctx);
        assert!(out.contains("tool.risk") && out.contains("agent.wakeup"), "{out}");
        assert!(out.contains("ms"), "{out}");
        assert!(ctx.store.outcomes().contains_key("ok"));
    }

    fn render_wide(d: &mut Dashboard, ctx: &Ctx) -> String {
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| d.draw(ctx, f, f.area())).unwrap();
        term.backend().to_string()
    }

    /// Lleva la selección de la topología al puerto `name` con `n` y pulsa `enter`; devuelve lo que se ve después.
    fn open_port(name: &str) -> String {
        let now = Instant::now();
        let mut app = crate::app::App::new(Box::new(settled()), FxMode::Events, now);
        // Los eventos van por el camino de la aplicación, para que ella sepa de flujos y capacidades.
        let (flows, wakeup) = (crate::fake::flows(), crate::fake::wakeup_flow());
        let announced = crate::fake::loaded_events(&flows)
            .into_iter()
            .chain(crate::fake::loaded_events(std::slice::from_ref(&wakeup)))
            .chain(crate::fake::capability_events());
        for e in announced {
            app.update(Msg::Event(Box::new(e)));
        }
        let mut rng = crate::fake::Rng::new(5);
        for i in 0..6 {
            for (_, e) in crate::fake::hub_trace(&mut rng, &format!("h{i}"), "crush", &wakeup, None) {
                app.update(Msg::Event(Box::new(e)));
            }
        }
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        for _ in 0..24 {
            app.update(press(KeyCode::Char('n')));
            term.draw(|f| app.draw(f)).unwrap();
            // El título del panel de métricas dice qué nodo está seleccionado.
            if term.backend().to_string().contains(&format!("{name} ·")) {
                break;
            }
        }
        app.update(press(KeyCode::Enter));
        term.draw(|f| app.draw(f)).unwrap();
        term.backend().to_string()
    }

    #[test]
    fn enter_on_an_exec_capability_opens_its_card() {
        let out = open_port("tool.risk");
        assert!(out.contains("proveedor toolgate · exec") && out.contains("últimas respuestas"), "{out}");
    }

    #[test]
    fn enter_on_a_flow_capability_opens_its_flow() {
        let out = open_port("agent.wakeup");
        assert!(out.contains("agent-wakeup") && out.contains("Wakegate como flujo"), "{out}");
    }

    fn press(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn loaded() -> Ctx {
        let now = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, now);
        for e in crate::fake::loaded_events(&crate::fake::flows()) {
            ctx.topo.reduce(&e, now);
        }
        ctx
    }

    #[test]
    fn enter_on_a_flow_port_opens_its_screen() {
        let now = Instant::now();
        let mut app = crate::app::App::new(Box::new(settled()), FxMode::Events, now);
        for e in crate::fake::loaded_events(&crate::fake::flows()) {
            app.update(Msg::Event(Box::new(e)));
        }
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        // `n` recorre los nodos de la topología; algún puerto de flujo acabará seleccionado.
        for _ in 0..16 {
            app.update(press(KeyCode::Char('n')));
            app.update(press(KeyCode::Enter));
            if app.depth() == 2 {
                break;
            }
        }
        assert_eq!(app.depth(), 2);
        app.update(press(KeyCode::Esc));
        assert_eq!(app.depth(), 1);
    }

    #[test]
    fn enter_on_passthrough_does_nothing() {
        let mut d = settled();
        let mut ctx = loaded();
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| d.draw(&ctx, f, f.area())).unwrap();
        for _ in 0..16 {
            d.update(&mut ctx, &press(KeyCode::Char('n')));
            if d.topo_view.selected() == Some(&NodeId::Port(PASSTHROUGH.into())) {
                break;
            }
        }
        assert_eq!(d.topo_view.selected(), Some(&NodeId::Port(PASSTHROUGH.into())));
        assert!(d.update(&mut ctx, &press(KeyCode::Enter)).is_none());
    }

    #[test]
    fn enter_opens_the_trace_and_esc_returns() {
        let now = Instant::now();
        let mut app = crate::app::App::new(Box::new(Dashboard::new()), FxMode::Events, now);
        app.update(Msg::Event(Box::new(decision_event("t1", "allow"))));
        app.update(Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(app.depth(), 2);
        app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(app.depth(), 1);
    }

    #[test]
    fn startup_coalesce_runs_once_and_off_never_animates() {
        let t0 = Instant::now();
        for (mode, expected) in [(FxMode::Off, false), (FxMode::Events, true)] {
            let ctx = Ctx::new(mode, t0);
            let mut d = Dashboard::new();
            render(&mut d, &ctx);
            assert_eq!(d.animating(), expected, "modo {mode}");
        }
        let mut d = Dashboard::new();
        let mut ctx = Ctx::new(FxMode::Events, t0);
        render(&mut d, &ctx);
        ctx.now = t0 + std::time::Duration::from_secs(2);
        let out = render(&mut d, &ctx);
        assert!(!d.animating());
        assert!(out.contains("jevalan · fake"));
    }

    #[test]
    fn a_deny_cue_flashes_the_port_and_then_settles() {
        let t0 = Instant::now();
        let mut d = settled();
        let mut ctx = Ctx::new(FxMode::Events, t0);
        for e in crate::fake::loaded_events(&crate::fake::flows()) {
            ctx.topo.reduce(&e, t0);
        }
        render(&mut d, &ctx);
        assert!(!d.animating());
        ctx.cues.push(crate::topology::Cue::Denied("agent-guard".into()));
        d.update(&mut ctx, &Msg::Tick(t0));
        assert!(d.animating());
        ctx.cues.clear();
        render(&mut d, &ctx);
        ctx.now = t0 + std::time::Duration::from_secs(1);
        render(&mut d, &ctx);
        assert!(!d.animating());
    }

    type Calls = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

    fn with_toggle() -> (Ctx, Calls) {
        let calls = Calls::default();
        let sink = calls.clone();
        let mut ctx = loaded();
        ctx.toggle = Some(Box::new(move |name| {
            sink.lock().unwrap().push(name.to_string());
            Some(true)
        }));
        (ctx, calls)
    }

    /// Dibuja y recorre los nodos con `n` hasta tener un backend seleccionado.
    fn select_backend(d: &mut Dashboard, ctx: &mut Ctx) -> String {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| d.draw(ctx, f, f.area())).unwrap();
        for _ in 0..24 {
            d.update(ctx, &press(KeyCode::Char('n')));
            if let Some(NodeId::Backend(name)) = d.topo_view.selected() {
                return name.clone();
            }
        }
        panic!("ningún backend seleccionable");
    }

    #[test]
    fn x_toggles_the_selected_backend() {
        let mut d = settled();
        let (mut ctx, calls) = with_toggle();
        let name = select_backend(&mut d, &mut ctx);
        d.update(&mut ctx, &press(KeyCode::Char('x')));
        assert_eq!(calls.lock().unwrap().as_slice(), std::slice::from_ref(&name));
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert!(text(header(&ctx)).contains(&format!("{name} apagado (x para encender)")));
    }

    #[test]
    fn x_without_toggle_does_nothing() {
        let mut d = settled();
        let mut ctx = loaded();
        select_backend(&mut d, &mut ctx);
        assert!(d.update(&mut ctx, &press(KeyCode::Char('x'))).is_none());
        assert!(ctx.notice.is_none());
    }

    #[test]
    fn x_hint_only_with_toggle() {
        let mut d = settled();
        let (with, _) = with_toggle();
        assert!(render(&mut d, &with).contains("x apagar"));
        assert!(!render(&mut d, &loaded()).contains("x apagar"));
    }
}
