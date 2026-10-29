//! Flow screen: a flow's graph with its travelled branches.

use crate::screen::{Ctx, KeyHint, Msg, Nav, Screen};
use crate::screens::dashboard::footer;
use crate::theme;
use crate::widgets::flow_graph::FlowGraph;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

const KEYS: &[KeyHint] = &[
    KeyHint { key: "←↑↓→ hjkl", desc: "mover la vista" },
    KeyHint { key: "+ -", desc: "zoom" },
    KeyHint { key: "0", desc: "ajustar a la vista" },
    KeyHint { key: "esc", desc: "volver" },
];

pub struct FlowScreen {
    flow: String,
    graph: FlowGraph,
    area: Rect,
}

impl FlowScreen {
    pub fn new(flow: String) -> Self {
        Self { flow, graph: FlowGraph::new(), area: Rect::default() }
    }
}

impl Screen for FlowScreen {
    fn update(&mut self, _ctx: &mut Ctx, msg: &Msg) -> Option<Nav> {
        if matches!(msg, Msg::Key(_) | Msg::Mouse(_)) {
            self.graph.handle(msg, self.area);
        }
        None
    }

    fn draw(&mut self, ctx: &Ctx, f: &mut Frame, area: Rect) {
        let [head, body, foot] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(area);
        self.area = body;
        let Some(flow) = ctx.flows.get(&self.flow) else {
            f.render_widget(Paragraph::new(Line::styled(self.flow.as_str(), theme::title())), head);
            let row = Rect::new(body.x, body.y + body.height / 2, body.width, 1.min(body.height));
            f.render_widget(
                Paragraph::new("flujo no disponible · esc para volver")
                    .style(theme::dim())
                    .alignment(Alignment::Center),
                row,
            );
            return;
        };
        let title = Line::from(vec![
            Span::styled(self.flow.as_str(), theme::title()),
            Span::styled(format!(" · {}", flow.description), theme::text()),
        ]);
        f.render_widget(Paragraph::new(title), head);
        self.graph.render(flow, &ctx.activity, ctx.now, body, f.buffer_mut());
        f.render_widget(Paragraph::new(footer(KEYS)), foot);
    }

    fn keys(&self) -> &'static [KeyHint] {
        KEYS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxMode;
    use jevalan_events::{Event, EventKind, new_event};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn node_end(node: &str, answer: serde_json::Value, confidence: f64, port: &str) -> Event {
        let kind =
            EventKind::NodeEnd { flow: "agent-guard".into(), node: node.into(), answer, confidence, port: port.into() };
        new_event("t", 1, "test", kind)
    }

    fn loaded_ctx(now: Instant) -> Ctx {
        let mut ctx = Ctx::new(FxMode::Events, now);
        for f in crate::fake::flows() {
            ctx.flows.insert(f.name.clone(), std::sync::Arc::new(f));
        }
        ctx
    }

    fn render(screen: &mut FlowScreen, ctx: &Ctx, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| screen.draw(ctx, f, f.area())).unwrap();
        term.backend().to_string()
    }

    #[test]
    fn agent_guard_with_activity_snapshot() {
        let t0 = Instant::now();
        let mut ctx = loaded_ctx(t0);
        let flows = ctx.flows.clone();
        for (node, answer, conf, port, times) in [
            ("destructive", json!("false"), 0.93, "false", 12),
            ("kind", json!("shell"), 0.88, "shell", 7),
            ("kind", json!("code-edit"), 0.91, "code-edit", 5),
            ("risk", json!(2.0), 0.74, "1.5..3", 3),
            ("risk", json!(0.5), 0.8, "0..1.5", 4),
        ] {
            for _ in 0..times {
                ctx.activity.on_event(&flows, &node_end(node, answer.clone(), conf, port), t0);
            }
        }
        // The last packet is halfway along the kind -> risk edge.
        ctx.activity.on_event(&flows, &node_end("kind", json!("shell"), 0.88, "shell"), t0);
        ctx.now = t0 + Duration::from_millis(300);
        insta::assert_snapshot!(render(&mut FlowScreen::new("agent-guard".into()), &ctx, 100, 24));
    }

    #[test]
    fn model_route_renders_every_box() {
        let ctx = loaded_ctx(Instant::now());
        let out = render(&mut FlowScreen::new("model-route".into()), &ctx, 100, 24);
        for label in ["score · tier", "noul · risky", "small", "medium", "large", "route"] {
            assert!(out.contains(label), "falta {label:?} en\n{out}");
        }
    }

    #[test]
    fn batch_node_renders_in_the_flow_screen() {
        let mut ctx = loaded_ctx(Instant::now());
        let batch = crate::fake::batch_flow();
        ctx.flows.insert(batch.name.clone(), std::sync::Arc::new(batch));
        let out = render(&mut FlowScreen::new("tool-risk".into()), &ctx, 100, 24);
        assert!(out.contains("batch · risks"), "falta el nodo batch en\n{out}");
        // `route` destinations are not `out` edges: the drawing only wires what the YAML wires.
        assert!(out.contains("abstain"), "falta la salida del puerto de error en\n{out}");
        assert!(out.contains("⠤error"), "falta la arista del puerto de error en\n{out}");
    }

    #[test]
    fn missing_flow_shows_a_notice() {
        let ctx = Ctx::new(FxMode::Events, Instant::now());
        let out = render(&mut FlowScreen::new("fantasma".into()), &ctx, 100, 24);
        assert!(out.contains("flujo no disponible"), "{out}");
        assert!(out.contains("fantasma"));
    }
}
