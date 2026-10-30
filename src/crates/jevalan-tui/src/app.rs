//! Elm-style shell of the TUI: a state, messages, `update` and `draw`.

use crate::fx::FxMode;
use crate::screen::{Ctx, KeyHint, Msg, Nav, Screen, SourceStatus};
use crate::theme;
use jevalan_events::{Event, EventKind};
use jevalan_flow::Flow;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
/// Events kept while paused; once exceeded, the oldest are dropped.
pub const MAX_PAUSED: usize = 10_000;

pub const GLOBAL_KEYS: &[KeyHint] = &[
    KeyHint { key: "q", desc: "salir" },
    KeyHint { key: "?", desc: "ayuda" },
    KeyHint { key: "f", desc: "efectos off / events / full" },
    KeyHint { key: "p", desc: "pausar / reanudar" },
    KeyHint { key: "esc", desc: "atrás / salir" },
];

pub struct App {
    pub ctx: Ctx,
    stack: Vec<Box<dyn Screen>>,
    pub help: bool,
    pub quit: bool,
    queued: VecDeque<Event>,
    dirty: bool,
}

impl App {
    pub fn new(root: Box<dyn Screen>, fx: FxMode, now: Instant) -> Self {
        Self {
            ctx: Ctx::new(fx, now),
            stack: vec![root],
            help: false,
            quit: false,
            queued: VecDeque::new(),
            dirty: true,
        }
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    pub fn queued(&self) -> usize {
        self.queued.len()
    }

    pub fn update(&mut self, msg: Msg) {
        match &msg {
            Msg::Key(k) if k.kind != KeyEventKind::Press => return,
            Msg::Key(k) => {
                self.dirty = true;
                if self.global_key(k) {
                    return;
                }
            }
            Msg::Event(e) => {
                if self.ctx.paused {
                    if self.queued.len() == MAX_PAUSED {
                        self.queued.pop_front();
                        self.ctx.store.note_dropped();
                    }
                    self.queued.push_back((**e).clone());
                } else {
                    self.apply((**e).clone());
                }
                self.dirty = true;
                return;
            }
            Msg::Status(status) => {
                self.ctx.source_status = Some(status.clone());
                if *status == SourceStatus::Connected {
                    // Flows and capabilities are state: the daemon resends them on connecting, and what is local may be out of date.
                    self.ctx.flows.clear();
                    self.ctx.topo.clear_flow_ports();
                    self.ctx.topo.clear_capabilities();
                    self.ctx.activity.clear_flows();
                }
                self.dirty = true;
                return;
            }
            Msg::Tick(now) => {
                self.ctx.now = *now;
                self.ctx.activity.tick(*now);
                let cues = self.ctx.topo.tick(*now);
                self.dirty |= !cues.is_empty();
                self.ctx.cues.extend(cues);
            }
            Msg::Mouse(_) | Msg::Resize(..) => self.dirty = true,
        }
        let top = self.stack.last_mut().expect("siempre hay pantalla raíz");
        match top.update(&mut self.ctx, &msg) {
            Some(Nav::Push(screen)) => {
                self.stack.push(screen);
                self.dirty = true;
            }
            Some(Nav::Pop) => self.pop(),
            None => {}
        }
        if matches!(msg, Msg::Tick(_)) {
            self.ctx.cues.clear();
        }
    }

    /// Global keys; returns `true` if the key was handled.
    fn global_key(&mut self, k: &KeyEvent) -> bool {
        if self.help {
            if matches!(k.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.help = false;
            }
            return true;
        }
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('f') => self.ctx.fx = self.ctx.fx.next(),
            KeyCode::Char('p') => {
                self.ctx.paused = !self.ctx.paused;
                if !self.ctx.paused {
                    while let Some(e) = self.queued.pop_front() {
                        self.apply(e);
                    }
                }
            }
            KeyCode::Esc if self.stack.len() > 1 => self.pop(),
            // At the root there is nowhere to go back to: `esc` quits, as in any other viewer.
            KeyCode::Esc => self.quit = true,
            _ => return false,
        }
        true
    }

    fn pop(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
            self.dirty = true;
        }
    }

    fn apply(&mut self, e: Event) {
        let now = self.ctx.now;
        // Announcing backends and loading or removing flows and capabilities is idempotent: it is applied even if the trace was already recorded.
        let fresh = self.ctx.store.ingest(e.clone(), now);
        let is_state = matches!(
            e.kind,
            EventKind::Backends { .. }
                | EventKind::FlowLoaded { .. }
                | EventKind::FlowRemoved { .. }
                | EventKind::CapabilityLoaded { .. }
                | EventKind::CapabilityInvalid { .. }
                | EventKind::CapabilityRemoved { .. }
        );
        if fresh || is_state {
            let cues = self.ctx.topo.reduce(&e, now);
            self.ctx.cues.extend(cues);
            self.track_flows(&e.kind);
            self.ctx.activity.on_event(&self.ctx.flows, &e, now);
        }
    }

    /// Keeps `ctx.flows` with the structure that arrives in `FLOW_LOADED`; an unreadable definition is ignored.
    fn track_flows(&mut self, kind: &EventKind) {
        match kind {
            EventKind::FlowLoaded { flow, definition, .. } => {
                if let Ok(def) = serde_json::from_value::<Flow>(definition.clone()) {
                    self.ctx.flows.insert(flow.clone(), Arc::new(def));
                }
            }
            EventKind::FlowRemoved { flow } => {
                self.ctx.flows.remove(flow);
            }
            _ => {}
        }
    }

    /// The event source closed; the header gives notice.
    pub fn mark_source_closed(&mut self) {
        if !self.ctx.source_closed {
            self.ctx.source_closed = true;
            self.dirty = true;
        }
    }

    /// Painting is needed if something changed, there are packets in flight, the screen animates or the effects are at full tilt.
    pub fn needs_redraw(&self) -> bool {
        self.dirty
            || self.ctx.topo.has_live_packets()
            || self.ctx.activity.has_packets()
            || self.ctx.fx == FxMode::Full
            || self.stack.last().is_some_and(|s| s.animating())
    }

    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        self.dirty = false;
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            let msg = Paragraph::new("terminal demasiado pequeño").alignment(Alignment::Center);
            f.render_widget(msg, area);
            return;
        }
        let top = self.stack.last_mut().expect("siempre hay pantalla raíz");
        top.draw(&self.ctx, f, area);
        if self.help {
            draw_help(f, area, top.keys());
        }
    }
}

fn draw_help(f: &mut Frame, area: Rect, keys: &[KeyHint]) {
    let lines: Vec<Line> = GLOBAL_KEYS
        .iter()
        .chain(keys)
        .map(|k| Line::from(vec![Span::styled(format!("{:>9}  ", k.key), theme::accent()), Span::raw(k.desc)]))
        .collect();
    // As wide as the longest line plus the borders, never wider than the screen.
    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let w = (widest + 2).max(48).min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(lines).block(Block::bordered().title(" ayuda ").title_style(theme::title())), popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::Cue;
    use jevalan_events::{Endpoint, SCHEMA_VERSION};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Test screen: notes what it receives; `x` stacks another like it, `z` asks to quit.
    struct Probe {
        seen: Rc<RefCell<Vec<String>>>,
    }

    impl Screen for Probe {
        fn update(&mut self, _ctx: &mut Ctx, msg: &Msg) -> Option<Nav> {
            if let Msg::Key(k) = msg {
                self.seen.borrow_mut().push(format!("{:?}", k.code));
                return match k.code {
                    KeyCode::Char('x') => Some(Nav::Push(Box::new(Probe { seen: self.seen.clone() }))),
                    KeyCode::Char('z') => Some(Nav::Pop),
                    _ => None,
                };
            }
            None
        }

        fn draw(&mut self, _ctx: &Ctx, f: &mut Frame, area: Rect) {
            f.render_widget(Paragraph::new("probe"), area);
        }

        fn keys(&self) -> &'static [KeyHint] {
            &[KeyHint { key: "x", desc: "apilar" }]
        }
    }

    fn app() -> (App, Rc<RefCell<Vec<String>>>) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        (App::new(Box::new(Probe { seen: seen.clone() }), FxMode::Events, Instant::now()), seen)
    }

    fn key(c: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(c, KeyModifiers::NONE))
    }

    fn req(trace: &str) -> Msg {
        Msg::Event(Box::new(Event {
            v: SCHEMA_VERSION,
            id: trace.into(),
            ts: String::new(),
            trace_id: trace.into(),
            seq: 1,
            source: "test".into(),
            kind: EventKind::RequestReceived {
                client: "crush".into(),
                endpoint: Endpoint::Decide,
                flow: Some("agent-guard".into()),
                backend: None,
            },
        }))
    }

    fn flow_event(trace: &str, kind: EventKind) -> Msg {
        Msg::Event(Box::new(Event {
            v: SCHEMA_VERSION,
            id: trace.into(),
            ts: String::new(),
            trace_id: trace.into(),
            seq: 1,
            source: "test".into(),
            kind,
        }))
    }

    #[test]
    fn push_and_pop_screens() {
        let (mut a, _) = app();
        a.update(key(KeyCode::Char('x')));
        assert_eq!(a.depth(), 2);
        a.update(key(KeyCode::Esc));
        assert_eq!(a.depth(), 1);
        assert!(!a.quit, "volver atrás no es salir");
        a.update(key(KeyCode::Char('z')));
        assert_eq!(a.depth(), 1, "la raíz no se puede sacar");
    }

    #[test]
    fn esc_at_the_root_quits() {
        let (mut a, _) = app();
        a.update(key(KeyCode::Esc));
        assert!(a.quit, "en la raíz no hay a dónde volver: esc sale");
    }

    #[test]
    fn esc_closes_the_help_without_quitting() {
        let (mut a, _) = app();
        a.update(key(KeyCode::Char('?')));
        a.update(key(KeyCode::Esc));
        assert!(!a.help);
        assert!(!a.quit, "cerrar la ayuda no es salir");
    }

    #[test]
    fn global_keys_are_not_forwarded() {
        let (mut a, seen) = app();
        a.update(key(KeyCode::Char('f')));
        assert_eq!(a.ctx.fx, FxMode::Full);
        a.update(key(KeyCode::Char('n')));
        assert_eq!(*seen.borrow(), vec!["Char('n')"]);
        a.update(key(KeyCode::Char('q')));
        assert!(a.quit);
    }

    #[test]
    fn help_swallows_keys_until_closed() {
        let (mut a, seen) = app();
        a.update(key(KeyCode::Char('?')));
        assert!(a.help);
        a.update(key(KeyCode::Char('q')));
        assert!(!a.quit, "con la ayuda abierta, q no sale");
        a.update(key(KeyCode::Esc));
        assert!(!a.help);
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn pause_queues_events_and_resume_applies_them() {
        let (mut a, _) = app();
        a.update(key(KeyCode::Char('p')));
        a.update(req("t1"));
        assert_eq!(a.queued(), 1);
        assert!(a.ctx.store.trace("t1").is_none());
        a.update(key(KeyCode::Char('p')));
        assert_eq!(a.queued(), 0);
        assert!(a.ctx.store.trace("t1").is_some());
        assert_eq!(a.ctx.cues, vec![Cue::ClientJoined("crush".into())]);
        a.update(Msg::Tick(a.ctx.now));
        assert!(a.ctx.cues.is_empty(), "los cues caducan con el tick");
    }

    #[test]
    fn flow_events_update_the_known_flows() {
        let (mut a, _) = app();
        for e in crate::fake::loaded_events(&crate::fake::flows()) {
            a.update(Msg::Event(Box::new(e)));
        }
        assert_eq!(a.ctx.flows.keys().collect::<Vec<_>>(), ["agent-guard", "model-route"]);
        assert_eq!(a.ctx.topo.ports(), ["agent-guard", "model-route", "passthrough"]);
        let loaded = crate::fake::loaded_events(&crate::fake::flows());
        a.update(Msg::Event(Box::new(loaded[0].clone())));
        assert_eq!(a.ctx.flows.len(), 2, "un FLOW_LOADED repetido no duplica");
        let bad = EventKind::FlowLoaded { flow: "rota".into(), nodes: 0, definition: serde_json::json!({"x": 1}) };
        a.update(flow_event("flow:rota:1", bad));
        assert!(!a.ctx.flows.contains_key("rota"), "una definición ilegible se ignora");
        a.update(flow_event("r", EventKind::FlowRemoved { flow: "agent-guard".into() }));
        assert_eq!(a.ctx.flows.keys().collect::<Vec<_>>(), ["model-route"]);
    }

    /// `FLOW_LOADED` of `agent-guard` with the given description and the `(trace_id, seq)` fixed.
    fn guard_loaded(description: &str) -> Msg {
        let mut def = serde_json::to_value(&crate::fake::flows()[0]).unwrap();
        def["description"] = description.into();
        let kind = EventKind::FlowLoaded { flow: "agent-guard".into(), nodes: 1, definition: def };
        flow_event("flow:agent-guard:1", kind)
    }

    #[test]
    fn backends_is_applied_even_if_seen() {
        let (mut a, _) = app();
        let kind = EventKind::Backends { backends: vec!["ollama".into()] };
        a.update(flow_event("backends:1", kind.clone()));
        assert_eq!(a.ctx.topo.backends().count(), 1);
        a.update(flow_event("backends:1", kind));
        assert_eq!(a.ctx.topo.backends().count(), 1);
        // A restarted daemon repeats the same (trace, seq) with other names.
        a.update(flow_event("backends:1", EventKind::Backends { backends: vec!["ollama".into(), "nuevo".into()] }));
        assert_eq!(a.ctx.topo.backends().count(), 2);
    }

    #[test]
    fn replayed_flow_loaded_is_idempotent() {
        let (mut a, _) = app();
        let loaded = crate::fake::loaded_events(&crate::fake::flows());
        let first =
            loaded.iter().find(|e| matches!(&e.kind, EventKind::FlowLoaded { flow, .. } if flow == "agent-guard"));
        let first = first.unwrap().clone();
        a.update(Msg::Event(Box::new(first.clone())));
        a.update(Msg::Event(Box::new(first)));
        assert_eq!(a.ctx.flows.len(), 1);
        assert_eq!(a.ctx.topo.ports(), ["agent-guard", "passthrough"]);
    }

    #[test]
    fn reconnect_resyncs_flows() {
        let (mut a, _) = app();
        let loaded = crate::fake::loaded_events(&crate::fake::flows());
        for e in &loaded {
            a.update(Msg::Event(Box::new(e.clone())));
        }
        assert_eq!(a.ctx.flows.len(), 2);
        a.update(Msg::Status(SourceStatus::Connected));
        assert!(a.ctx.flows.is_empty());
        let again =
            loaded.iter().find(|e| matches!(&e.kind, EventKind::FlowLoaded { flow, .. } if flow == "agent-guard"));
        a.update(Msg::Event(Box::new(again.unwrap().clone())));
        assert_eq!(a.ctx.flows.keys().collect::<Vec<_>>(), ["agent-guard"]);
        assert_eq!(a.ctx.topo.ports(), ["agent-guard", "passthrough"]);
    }

    fn capability_loaded(name: &str, provider: &str, kind: &str) -> Msg {
        let kind = EventKind::CapabilityLoaded {
            capability: name.into(),
            version: "1".into(),
            provider: provider.into(),
            provider_type: kind.into(),
            flow: None,
            description: String::new(),
            input_fields: vec![],
            decision_fields: vec![],
        };
        flow_event(&format!("capability:{name}:1"), kind)
    }

    #[test]
    fn capability_loaded_is_idempotent() {
        let (mut a, _) = app();
        for _ in 0..2 {
            for e in crate::fake::capability_events() {
                a.update(Msg::Event(Box::new(e)));
            }
        }
        assert_eq!(a.ctx.topo.capabilities().count(), 2, "un CAPABILITY_LOADED repetido no duplica");
        assert_eq!(a.ctx.topo.ports(), ["agent.wakeup", "tool.risk", "passthrough"]);
        assert!(a.ctx.topo.backends().any(|(n, _)| n == "toolgate"));
        // A restarted daemon repeats the same (trace, seq) with another version: it is applied even if it was already recorded.
        let kind = EventKind::CapabilityLoaded {
            capability: "tool.risk".into(),
            version: "2".into(),
            provider: "toolgate".into(),
            provider_type: "exec".into(),
            flow: None,
            description: String::new(),
            input_fields: vec![],
            decision_fields: vec![],
        };
        a.update(flow_event("capability:tool.risk:1", kind));
        assert_eq!(a.ctx.topo.capability("tool.risk").map(|c| c.version.as_str()), Some("2"));
        a.update(flow_event("r", EventKind::CapabilityRemoved { capability: "tool.risk".into() }));
        assert_eq!(a.ctx.topo.ports(), ["agent.wakeup", "passthrough"]);
    }

    #[test]
    fn connected_resyncs_capabilities() {
        let (mut a, _) = app();
        a.update(capability_loaded("tool.risk", "toolgate", "exec"));
        a.update(capability_loaded("agent.wakeup", "jevalan-flow", "flow"));
        assert_eq!(a.ctx.topo.capabilities().count(), 2);
        a.update(Msg::Status(SourceStatus::Connected));
        assert_eq!(a.ctx.topo.capabilities().count(), 0, "lo local puede estar obsoleto");
        assert_eq!(a.ctx.topo.backends().count(), 0, "y el chip del especialista con ello");
        a.update(capability_loaded("agent.wakeup", "jevalan-flow", "flow"));
        assert_eq!(a.ctx.topo.ports(), ["agent.wakeup", "passthrough"]);
    }

    #[test]
    fn restarted_daemon_updates_a_flow() {
        let (mut a, _) = app();
        a.update(guard_loaded("X"));
        a.update(Msg::Status(SourceStatus::Connected));
        a.update(guard_loaded("Y"));
        assert_eq!(a.ctx.flows["agent-guard"].description, "Y");
    }

    #[test]
    fn status_updates_the_header() {
        let (mut a, _) = app();
        assert_eq!(a.ctx.source_status, None);
        a.update(Msg::Status(SourceStatus::Connected));
        assert_eq!(a.ctx.source_status, Some(SourceStatus::Connected));
        a.update(Msg::Status(SourceStatus::Disconnected("cerrado".into())));
        let text: String =
            crate::screens::dashboard::header(&a.ctx).spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("sin conexión: cerrado (reintentando)"), "{text}");
    }

    #[test]
    fn long_pause_drops_oldest_events() {
        let (mut a, _) = app();
        a.update(key(KeyCode::Char('p')));
        for i in 0..MAX_PAUSED + 3 {
            a.update(req(&format!("t{i}")));
        }
        assert_eq!(a.queued(), MAX_PAUSED);
        assert_eq!(a.ctx.store.dropped(), 3);
    }

    #[test]
    fn redraw_only_when_needed() {
        let (mut a, _) = app();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        assert!(!a.needs_redraw(), "en reposo no se pinta");
        a.update(req("t1"));
        assert!(a.needs_redraw());
        term.draw(|f| a.draw(f)).unwrap();
        assert!(a.needs_redraw(), "hay un paquete en vuelo");
        a.update(Msg::Tick(a.ctx.now + crate::topology::PACKET_TTL));
        term.draw(|f| a.draw(f)).unwrap();
        assert!(!a.needs_redraw());
        a.update(key(KeyCode::Char('f')));
        term.draw(|f| a.draw(f)).unwrap();
        assert!(a.needs_redraw(), "fx=full pinta siempre");
    }

    #[test]
    fn tiny_terminal_shows_a_notice() {
        let (mut a, _) = app();
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        let text: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("terminal demasiado pequeño"));
    }

    #[test]
    fn help_overlay_grows_to_show_a_long_description_whole() {
        let long = "elegir decisión y abrir su traza (feed) con una descripción bien larga";
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw_help(f, f.area(), &[KeyHint { key: "↑↓ enter", desc: long }])).unwrap();
        let text: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains(long), "la ayuda recortó la línea");
    }

    #[test]
    fn help_overlay_lists_global_and_screen_keys() {
        let (mut a, _) = app();
        a.update(key(KeyCode::Char('?')));
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| a.draw(f)).unwrap();
        let text: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("pausar / reanudar"));
        assert!(text.contains("apilar"));
    }
}
