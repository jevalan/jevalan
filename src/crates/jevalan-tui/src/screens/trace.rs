//! Trace screen: the node-by-node path (by default) or the event timeline.

use crate::fx::FxState;
use crate::screen::{Ctx, KeyHint, Msg, Nav, Screen};
use crate::theme;
use jevalan_events::{Event, EventKind};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table};
use serde_json::Value;
use std::time::Instant;

pub struct TraceScreen {
    trace_id: String,
    scroll: usize,
    /// `true` shows the event timeline instead of the node path.
    timeline: bool,
    fx: FxState,
    started: bool,
}

impl TraceScreen {
    pub fn new(trace_id: String) -> Self {
        Self { trace_id, scroll: 0, timeline: false, fx: FxState::new(), started: false }
    }
}

/// A node's answer in one word: a number with two decimals, text, or "—" if the backend failed.
pub fn answer_text(answer: &Value) -> String {
    match answer {
        Value::Null => "—".into(),
        Value::String(t) => t.clone(),
        Value::Number(n) => n.as_f64().map_or_else(|| n.to_string(), |x| format!("{x:.2}")),
        other => other.to_string(),
    }
}

/// What a `NODE_AUDIT` tells: the heuristics evaluated and the `route` rule that won (1 is the first).
fn audit_text(signals: &serde_json::Map<String, Value>, rule: Option<u32>) -> String {
    let mut parts: Vec<String> = signals.iter().map(|(k, v)| format!("{k}={v}")).collect();
    if let Some(r) = rule {
        parts.push(format!("regla {}", r + 1));
    }
    parts.join(" · ")
}

pub fn detail(kind: &EventKind) -> String {
    match kind {
        EventKind::RequestReceived { client, endpoint, flow, backend } => {
            let target = flow.as_deref().or(backend.as_deref()).unwrap_or("-");
            format!("cliente {client} · {endpoint:?} · {target}").to_lowercase()
        }
        EventKind::NodeStart { node_type, backend, model, .. } => format!("{node_type} · {backend}/{model}"),
        EventKind::BackendRequest { backend, questions, .. } => format!("→ {backend} · {questions} preguntas"),
        EventKind::BackendResponse { backend, status, duration_ms, .. } => {
            format!("{backend} · {status} · {duration_ms} ms")
        }
        EventKind::NodeEnd { node, answer, confidence, port, .. } => {
            format!("{node} · {} · conf {confidence:.2} · → {port}", answer_text(answer))
        }
        EventKind::NodeAudit { node, signals, rule, .. } => format!("{node} · {}", audit_text(signals, *rule)),
        EventKind::DecisionMade { outcome, path_len, duration_ms, .. } => {
            format!("{outcome} · {path_len} pasos · {duration_ms} ms")
        }
        EventKind::PassthroughDone { backend, model, status, duration_ms } => {
            format!("{backend}/{model} · {status} · {duration_ms} ms")
        }
        EventKind::Backends { backends, .. } => format!("backends: {}", backends.join(", ")),
        EventKind::FlowLoaded { nodes, .. } => format!("{nodes} nodos"),
        EventKind::FlowInvalid { reason, .. } => reason.clone(),
        EventKind::FlowRemoved { .. } => "retirado".into(),
        EventKind::HubRequest { capability, capability_version, provider, .. } => {
            format!("{capability} v{capability_version} → {provider}")
        }
        EventKind::HubResponse { status, code, elapsed_ms, .. } => {
            let code = code.as_deref().map_or_else(String::new, |c| format!(" {c}"));
            format!("{status}{code} · {elapsed_ms} ms")
        }
        EventKind::CapabilityLoaded { capability, version, provider, provider_type, .. } => {
            format!("{capability} v{version} ({provider_type}: {provider})")
        }
        EventKind::CapabilityInvalid { capability, reason } => format!("{capability}: {reason}"),
        EventKind::CapabilityRemoved { capability } => capability.clone(),
        EventKind::Error { stage, message } => format!("{stage}: {message}"),
        EventKind::Lagged { dropped } => format!("perdidos {dropped}"),
    }
}

/// "node" column: the node, backend or flow the event refers to.
fn subject(kind: &EventKind) -> &str {
    match kind {
        EventKind::NodeStart { node, .. } | EventKind::NodeEnd { node, .. } | EventKind::NodeAudit { node, .. } => node,
        EventKind::BackendRequest { backend, .. }
        | EventKind::BackendResponse { backend, .. }
        | EventKind::PassthroughDone { backend, .. } => backend,
        EventKind::DecisionMade { flow, .. }
        | EventKind::FlowLoaded { flow, .. }
        | EventKind::FlowInvalid { flow, .. }
        | EventKind::FlowRemoved { flow } => flow,
        EventKind::Error { stage, .. } => stage,
        EventKind::HubRequest { capability, .. }
        | EventKind::HubResponse { capability, .. }
        | EventKind::CapabilityLoaded { capability, .. }
        | EventKind::CapabilityInvalid { capability, .. }
        | EventKind::CapabilityRemoved { capability } => capability,
        EventKind::RequestReceived { .. } | EventKind::Backends { .. } | EventKind::Lagged { .. } => "-",
    }
}

/// A path node already closed (or failed) with what is known about it.
struct PathNode<'a> {
    node: &'a str,
    ty: &'a str,
    backend: &'a str,
    model: &'a str,
    started: Instant,
    /// Duration of the backend's response, if it arrived.
    ms: Option<u64>,
    ok: bool,
}

fn node_line(n: &PathNode, end: Option<(&Value, f64, &str, Instant)>) -> (String, Style) {
    let (answer, conf, port, ms) = match end {
        Some((answer, conf, port, at)) => {
            // Without BACKEND_RESPONSE, the gap between NODE_START and NODE_END.
            let ms = n.ms.unwrap_or_else(|| at.saturating_duration_since(n.started).as_millis() as u64);
            (answer_text(answer), format!("conf {conf:.2}"), format!("→ {port}"), format!("{ms} ms"))
        }
        None if n.ms.is_some() && !n.ok => {
            ("—".into(), "conf —".into(), "→ —".into(), format!("{} ms", n.ms.unwrap_or(0)))
        }
        None => ("…".into(), "conf …".into(), "→ …".into(), "…".into()),
    };
    let failed = end.map_or(!n.ok, |(_, _, port, _)| port == "error");
    let style = if failed { theme::error() } else { theme::text() };
    let line = format!(
        "{:<14} {:<8} {:<26} {:<10} {:<10} {:<18} {}",
        n.node,
        n.ty,
        format!("{}·{}", n.backend, n.model),
        answer,
        conf,
        port,
        ms
    );
    (line, style)
}

/// One line per path node (from `NODE_START`/`NODE_END`) and a final one with the outcome.
/// A passthrough has no nodes: only its `passthrough · backend · model · status · ms` line remains.
fn path_lines(events: &[(Instant, Event)]) -> Vec<(String, Style)> {
    let mut lines = Vec::new();
    let mut open: Option<PathNode> = None;
    for (at, e) in events {
        match &e.kind {
            EventKind::NodeStart { node, node_type, backend, model, .. } => {
                if let Some(n) = open.take() {
                    lines.push(node_line(&n, None));
                }
                open = Some(PathNode { node, ty: node_type, backend, model, started: *at, ms: None, ok: true });
            }
            EventKind::BackendResponse { ok, duration_ms, .. } => {
                if let Some(n) = open.as_mut() {
                    (n.ms, n.ok) = (Some(*duration_ms), *ok);
                }
            }
            EventKind::NodeEnd { answer, confidence, port, .. } => {
                if let Some(n) = open.take() {
                    lines.push(node_line(&n, Some((answer, *confidence, port, *at))));
                }
            }
            EventKind::NodeAudit { signals, rule, .. } => {
                lines.push((format!("  ↳ {}", audit_text(signals, *rule)), theme::text()));
            }
            EventKind::DecisionMade { outcome, path_len, duration_ms, .. } => {
                lines.push((format!("{outcome} · {path_len} pasos · {duration_ms} ms"), theme::outcome(outcome)));
            }
            EventKind::PassthroughDone { backend, model, status, duration_ms } => {
                lines.push((format!("passthrough · {backend} · {model} · {status} · {duration_ms} ms"), theme::text()));
            }
            EventKind::HubResponse { capability, status, code, elapsed_ms, .. } => {
                let code = code.as_deref().map_or_else(String::new, |c| format!(" {c}"));
                lines.push((
                    format!("hub · {capability} · {status}{code} · {elapsed_ms} ms"),
                    theme::hub_status(status),
                ));
            }
            EventKind::Error { stage, message } => {
                if let Some(n) = open.take() {
                    lines.push(node_line(&n, None));
                }
                lines.push((format!("error · {stage}: {message}"), theme::error()));
            }
            _ => {}
        }
    }
    if let Some(n) = open {
        lines.push(node_line(&n, None));
    }
    lines
}

const KEYS: &[KeyHint] = &[
    KeyHint { key: "↑↓", desc: "desplazar" },
    KeyHint { key: "t", desc: "camino / línea temporal" },
    KeyHint { key: "esc", desc: "volver" },
];

impl Screen for TraceScreen {
    fn update(&mut self, ctx: &mut Ctx, msg: &Msg) -> Option<Nav> {
        match msg {
            Msg::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Esc => return Some(Nav::Pop),
                KeyCode::Char('t') => {
                    self.timeline = !self.timeline;
                    self.scroll = 0;
                }
                KeyCode::Up => {
                    if self.scroll > 0 {
                        self.scroll -= 1;
                    }
                }
                KeyCode::Down => {
                    if let Some(trace) = ctx.store.trace(&self.trace_id) {
                        let max = trace.events.len().saturating_sub(1);
                        if self.scroll < max {
                            self.scroll += 1;
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
        None
    }

    fn draw(&mut self, ctx: &Ctx, f: &mut Frame, area: Rect) {
        let Some(trace) = ctx.store.trace(&self.trace_id) else {
            let msg = "traza descartada (ya no está en memoria) · esc para volver";
            f.render_widget(
                Paragraph::new(msg).style(theme::dim()).alignment(ratatui::layout::Alignment::Center),
                area,
            );
            return;
        };

        let client = trace.client.as_deref().unwrap_or("?");
        let (verdict_text, verdict_style) = trace
            .events
            .iter()
            .rev()
            .find_map(|(_, e)| match &e.kind {
                EventKind::DecisionMade { outcome, .. } => Some((outcome.clone(), theme::outcome(outcome))),
                EventKind::PassthroughDone { status, .. } => Some((format!("passthrough {status}"), theme::text())),
                EventKind::Error { .. } => Some(("error".to_string(), theme::error())),
                EventKind::HubResponse { status, .. } => Some((format!("hub {status}"), theme::hub_status(status))),
                _ => None,
            })
            .unwrap_or_else(|| ("en curso".to_string(), theme::text()));

        let header = Line::from(vec![
            Span::styled(format!("traza {}", self.trace_id), theme::title()),
            Span::styled(" · cliente ", theme::text()),
            Span::styled(client, theme::accent()),
            Span::styled(" · ", theme::text()),
            Span::styled(verdict_text, verdict_style),
        ]);

        // Calculate header + footer + table rows
        let header_height = 1;
        let footer_height = 1;
        let table_area_height = area.height.saturating_sub(header_height + footer_height);

        // Draw header
        f.render_widget(Paragraph::new(header), Rect { x: area.x, y: area.y, width: area.width, height: 1 });

        let table_rect = Rect { x: area.x, y: area.y + header_height, width: area.width, height: table_area_height };
        if !self.timeline {
            let head = format!(
                "{:<14} {:<8} {:<26} {:<10} {:<10} {:<18} {}",
                "nodo", "tipo", "backend·modelo", "respuesta", "conf", "puerto", "ms"
            );
            let mut lines = vec![Line::styled(head, theme::accent())];
            lines.extend(path_lines(&trace.events).into_iter().map(|(l, st)| Line::styled(l, st)));
            let para = Paragraph::new(lines).block(Block::bordered()).scroll((self.scroll as u16, 0));
            f.render_widget(para, table_rect);
        } else {
            // Timeline: one row per event.
            let mut rows = Vec::new();
            let first_at = trace.events.first().map(|(at, _)| *at);

            for (at, event) in trace.events.iter() {
                let seq = format!("{}", event.seq);
                let offset_ms = first_at.map_or(0, |first| at.duration_since(first).as_millis());
                let time = format!("{}ms", offset_ms);
                let kind = event.kind.name();
                let subject = subject(&event.kind);
                let detail_str = detail(&event.kind);

                let style = match &event.kind {
                    EventKind::Error { .. } => theme::error(),
                    EventKind::DecisionMade { outcome, .. } => theme::outcome(outcome),
                    EventKind::HubResponse { status, .. } => theme::hub_status(status),
                    _ => theme::text(),
                };

                rows.push(Row::new(vec![seq, time, kind.to_string(), subject.to_string(), detail_str]).style(style));
            }

            let table = Table::new(
                rows,
                [
                    ratatui::layout::Constraint::Length(4),
                    ratatui::layout::Constraint::Length(7),
                    ratatui::layout::Constraint::Length(17),
                    ratatui::layout::Constraint::Length(17),
                    ratatui::layout::Constraint::Min(20),
                ],
            )
            .header(Row::new(vec!["seq", "+ms", "tipo", "nodo", "detalle"]).style(theme::accent()))
            .block(Block::bordered());

            f.render_widget(table, table_rect);
        }

        // Draw footer
        let mut footer_spans = Vec::new();
        for (i, k) in KEYS.iter().enumerate() {
            if i > 0 {
                footer_spans.push(Span::styled(" · ", theme::dim()));
            }
            footer_spans.push(Span::styled(k.key, theme::accent()));
            footer_spans.push(Span::styled(format!(" {}", k.desc), theme::dim()));
        }
        let footer = Line::from(footer_spans);

        f.render_widget(
            Paragraph::new(footer),
            Rect { x: area.x, y: area.y + area.height.saturating_sub(1), width: area.width, height: 1 },
        );

        if !self.started {
            self.started = true;
            self.fx.on_enter_screen(ctx.fx, area);
        }
        self.fx.process(ctx.fx, ctx.now, f.buffer_mut(), area);
    }

    fn animating(&self) -> bool {
        self.fx.is_running()
    }

    fn keys(&self) -> &'static [KeyHint] {
        KEYS
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn node_audit_cuenta_senales_y_regla() {
        let signals = serde_json::Map::from_iter([("rm_root".to_string(), Value::Bool(false))]);
        let kind = EventKind::NodeAudit { flow: "f".into(), node: "risks".into(), signals, rule: Some(1) };
        assert_eq!(detail(&kind), "risks · rm_root=false · regla 2");
    }

    use super::*;
    use crate::fx::FxMode;
    use crate::screen::Ctx;
    use jevalan_events::EventKind;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;
    use std::time::Instant;

    fn render(s: &mut TraceScreen, ctx: &Ctx) -> String {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| s.draw(ctx, f, f.area())).unwrap();
        term.backend().to_string()
    }

    #[test]
    fn details_per_kind() {
        let end = EventKind::NodeEnd {
            flow: "agent-guard".into(),
            node: "destructive".into(),
            answer: serde_json::json!(0.912),
            confidence: 0.93,
            port: "false".into(),
        };
        assert_eq!(detail(&end), "destructive · 0.91 · conf 0.93 · → false");
        let failed = EventKind::NodeEnd {
            flow: "f".into(),
            node: "kind".into(),
            answer: Value::Null,
            confidence: 0.0,
            port: "error".into(),
        };
        assert_eq!(detail(&failed), "kind · — · conf 0.00 · → error");
        let d = EventKind::DecisionMade { flow: "f".into(), outcome: "deny".into(), path_len: 2, duration_ms: 412 };
        assert_eq!(detail(&d), "deny · 2 pasos · 412 ms");
        let e = EventKind::Error { stage: "backend".into(), message: "destructive: timeout".into() };
        assert_eq!(detail(&e), "backend: destructive: timeout");
        let r = EventKind::BackendResponse {
            backend: "ollama".into(),
            ok: true,
            status: 200,
            duration_ms: 38,
            input_tokens: None,
        };
        assert_eq!(detail(&r), "ollama · 200 · 38 ms");
    }

    fn press_t(s: &mut TraceScreen, ctx: &mut Ctx) {
        let key =
            ratatui::crossterm::event::KeyEvent::new(KeyCode::Char('t'), ratatui::crossterm::event::KeyModifiers::NONE);
        s.update(ctx, &Msg::Key(key));
    }

    fn ctx_with(events: Vec<(u64, EventKind)>) -> Ctx {
        let now = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, now);
        for (i, (ms, kind)) in events.into_iter().enumerate() {
            let e = jevalan_events::new_event("t1", i as u64 + 1, "test", kind);
            ctx.store.ingest(e, now + std::time::Duration::from_millis(ms));
        }
        ctx
    }

    fn received(flow: bool) -> EventKind {
        let (endpoint, flow, backend) = if flow {
            (jevalan_events::Endpoint::Decide, Some("agent-guard".to_string()), None)
        } else {
            (jevalan_events::Endpoint::Systemone, None, Some("ollama".to_string()))
        };
        EventKind::RequestReceived { client: "crush".into(), endpoint, flow, backend }
    }

    fn node(name: &str, ty: &str, backend: &str) -> Vec<(u64, EventKind)> {
        let start = EventKind::NodeStart {
            flow: "agent-guard".into(),
            node: name.into(),
            node_type: ty.into(),
            backend: backend.into(),
            model: "nimble".into(),
        };
        let resp = EventKind::BackendResponse {
            backend: backend.into(),
            ok: true,
            status: 200,
            duration_ms: 38,
            input_tokens: None,
        };
        let end = EventKind::NodeEnd {
            flow: "agent-guard".into(),
            node: name.into(),
            answer: serde_json::json!("yes"),
            confidence: 0.93,
            port: "true".into(),
        };
        vec![(0, start), (1, resp), (2, end)]
    }

    fn decision() -> EventKind {
        EventKind::DecisionMade { flow: "agent-guard".into(), outcome: "deny".into(), path_len: 2, duration_ms: 412 }
    }

    #[test]
    fn path_view_lists_one_row_per_node() {
        let mut events = vec![(0, received(true))];
        events.extend(node("destructive", "choice", "ollama"));
        events.extend(node("kind", "choice", "typesafe"));
        events.push((5, decision()));
        let ctx = ctx_with(events);
        let out = render(&mut TraceScreen::new("t1".into()), &ctx);
        let rows: Vec<&str> = out.lines().filter(|l| l.contains("conf 0.93")).collect();
        assert_eq!(rows.len(), 2, "{out}");
        assert!(rows[0].contains("destructive") && rows[0].contains("ollama·nimble") && rows[0].contains("yes"));
        assert!(rows[0].contains("→ true") && rows[0].contains("38 ms"), "{}", rows[0]);
        assert!(out.contains("deny · 2 pasos · 412 ms"), "{out}");
        assert!(!out.contains("REQUEST_RECEIVED"));
        insta::assert_snapshot!(out);
    }

    #[test]
    fn failed_trace_ends_with_error_row() {
        let mut events = vec![(0, received(true))];
        events.extend(node("destructive", "choice", "ollama"));
        let start = EventKind::NodeStart {
            flow: "agent-guard".into(),
            node: "kind".into(),
            node_type: "choice".into(),
            backend: "typesafe".into(),
            model: "nimble".into(),
        };
        let resp = EventKind::BackendResponse {
            backend: "typesafe".into(),
            ok: false,
            status: 0,
            duration_ms: 800,
            input_tokens: None,
        };
        events.extend([
            (5, start),
            (6, resp),
            (7, EventKind::Error { stage: "backend".into(), message: "kind: timeout".into() }),
        ]);
        let out = render(&mut TraceScreen::new("t1".into()), &ctx_with(events));
        let lines: Vec<&str> = out.lines().collect();
        let kind = lines.iter().position(|l| l.contains("typesafe·nimble")).expect("fila del nodo fallido");
        assert!(lines[kind].contains("800 ms") && lines[kind].contains('—'), "{}", lines[kind]);
        assert!(lines[kind + 1].contains("error · backend: kind: timeout"), "{out}");
    }

    #[test]
    fn hub_trace_ends_with_the_hub_row_and_verdict() {
        let request = EventKind::HubRequest {
            capability: "tool.risk".into(),
            capability_version: "1".into(),
            provider: "toolgate".into(),
            client: "vscode".into(),
        };
        let response = EventKind::HubResponse {
            capability: "tool.risk".into(),
            provider: "toolgate".into(),
            status: "error".into(),
            code: Some("deadline_exceeded".into()),
            elapsed_ms: 5000,
        };
        let out = render(&mut TraceScreen::new("t1".into()), &ctx_with(vec![(0, request), (5000, response)]));
        assert!(out.contains("cliente vscode · hub error"), "{out}");
        assert!(out.contains("hub · tool.risk · error deadline_exceeded · 5000 ms"), "{out}");
    }

    #[test]
    fn passthrough_trace_is_one_row() {
        let done = EventKind::PassthroughDone {
            backend: "ollama".into(),
            model: "nimble".into(),
            status: 200,
            duration_ms: 7,
        };
        let out = render(&mut TraceScreen::new("t1".into()), &ctx_with(vec![(0, received(false)), (7, done)]));
        let rows: Vec<&str> = out.lines().filter(|l| l.contains("passthrough ·")).collect();
        assert_eq!(rows.len(), 1, "{out}");
        assert!(rows[0].contains("passthrough · ollama · nimble · 200 · 7 ms"), "{}", rows[0]);
    }

    #[test]
    fn t_toggles_the_timeline() {
        let mut events = vec![(0, received(true))];
        events.extend(node("destructive", "choice", "ollama"));
        events.push((5, decision()));
        let mut ctx = ctx_with(events);
        let mut s = TraceScreen::new("t1".into());
        assert!(!render(&mut s, &ctx).contains("REQUEST_RECEIVED"));
        press_t(&mut s, &mut ctx);
        let out = render(&mut s, &ctx);
        assert!(out.contains("REQUEST_RECEIVED") && out.contains("BACKEND_RESPONSE"), "{out}");
        press_t(&mut s, &mut ctx);
        assert!(!render(&mut s, &ctx).contains("REQUEST_RECEIVED"));
    }

    #[test]
    fn shows_the_timeline_of_a_trace() {
        let now = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, now);
        for (offset, e) in crate::fake::trace(&mut crate::fake::Rng::new(5), "t1", "crush", &crate::fake::flows(), None)
        {
            ctx.store.ingest(e, now + offset);
        }
        let mut screen = TraceScreen::new("t1".into());
        press_t(&mut screen, &mut ctx);
        let out = render(&mut screen, &ctx);
        assert!(out.contains("traza t1"));
        assert!(out.contains("cliente crush"));
        assert!(out.contains("REQUEST_RECEIVED"));
        assert!(out.contains("DECISION_MADE"));
        insta::assert_snapshot!(out);
    }

    #[test]
    fn transition_only_animates_in_full() {
        let now = Instant::now();
        for (mode, expected) in [(FxMode::Off, false), (FxMode::Events, false), (FxMode::Full, true)] {
            let mut ctx = Ctx::new(mode, now);
            for (offset, e) in
                crate::fake::trace(&mut crate::fake::Rng::new(5), "t1", "crush", &crate::fake::flows(), None)
            {
                ctx.store.ingest(e, now + offset);
            }
            let mut s = TraceScreen::new("t1".into());
            render(&mut s, &ctx);
            assert_eq!(s.animating(), expected, "modo {mode}");
        }
    }

    #[test]
    fn evicted_trace_shows_a_notice() {
        let ctx = Ctx::new(FxMode::Events, Instant::now());
        let out = render(&mut TraceScreen::new("gone".into()), &ctx);
        assert!(out.contains("traza descartada"));
    }
}
