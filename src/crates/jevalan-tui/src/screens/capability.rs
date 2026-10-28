//! Ficha de una capacidad servida por un especialista `exec`/`http`: lo que se sabe de ella y sus últimas respuestas.

use crate::screen::{Ctx, KeyHint, Msg, Nav, Screen};
use crate::screens::dashboard::footer;
use crate::theme;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table};

const KEYS: &[KeyHint] = &[KeyHint { key: "esc", desc: "volver" }];

pub struct CapabilityScreen {
    capability: String,
}

impl CapabilityScreen {
    pub fn new(capability: String) -> Self {
        Self { capability }
    }
}

/// La hora de un `ts` RFC 3339, o guiones si no hay.
fn clock(ts: &str) -> String {
    ts.get(11..19).unwrap_or("--:--:--").to_string()
}

impl Screen for CapabilityScreen {
    fn update(&mut self, _ctx: &mut Ctx, _msg: &Msg) -> Option<Nav> {
        None
    }

    fn draw(&mut self, ctx: &Ctx, f: &mut Frame, area: Rect) {
        let [head, info, body, foot] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(5), Constraint::Min(1), Constraint::Length(1)])
                .areas(area);
        let Some(cap) = ctx.topo.capability(&self.capability) else {
            f.render_widget(Paragraph::new(Line::styled(self.capability.as_str(), theme::title())), head);
            let row = Rect::new(body.x, body.y + body.height / 2, body.width, 1.min(body.height));
            f.render_widget(
                Paragraph::new("capacidad no disponible · esc para volver")
                    .style(theme::dim())
                    .alignment(Alignment::Center),
                row,
            );
            return;
        };
        let title = Line::from(vec![
            Span::styled(self.capability.as_str(), theme::title()),
            Span::styled(format!(" · v{}", cap.version), theme::text()),
        ]);
        f.render_widget(Paragraph::new(title), head);
        let provider = Line::from(vec![
            Span::styled("proveedor ", theme::dim()),
            Span::styled(cap.provider.as_str(), theme::provider()),
            Span::styled(format!(" · {}", cap.kind), theme::text()),
        ]);
        let totals = ctx.store.flow(&self.capability);
        let count = |status: &str| totals.and_then(|t| t.get(status)).copied().unwrap_or(0);
        let tally = Line::from(vec![
            Span::styled("respuestas ", theme::dim()),
            Span::styled(format!("ok {}", count("ok")), theme::hub_status("ok")),
            Span::styled(" · ", theme::dim()),
            Span::styled(format!("abstain {}", count("abstain")), theme::hub_status("abstain")),
            Span::styled(" · ", theme::dim()),
            Span::styled(format!("error {}", count("error")), theme::hub_status("error")),
        ]);
        let list = |label: &'static str, fields: &[String]| {
            let names = if fields.is_empty() { "-".to_string() } else { fields.join(", ") };
            Line::from(vec![Span::styled(label, theme::dim()), Span::styled(names, theme::text())])
        };
        let lines = vec![
            Line::styled(cap.description.as_str(), theme::text()),
            provider,
            tally,
            list("entrada ", &cap.input_fields),
            list("decisión ", &cap.decision_fields),
        ];
        f.render_widget(Paragraph::new(lines), info);

        let rows: Vec<Row> = ctx
            .store
            .hub_results(&self.capability)
            .map(|r| {
                let code = r.code.clone().unwrap_or_else(|| "-".into());
                Row::new(vec![clock(&r.ts), r.status.clone(), code, format!("{} ms", r.ms)])
                    .style(theme::hub_status(&r.status))
            })
            .collect();
        let widths = [Constraint::Length(10), Constraint::Length(9), Constraint::Length(22), Constraint::Min(8)];
        let table = Table::new(rows, widths)
            .header(Row::new(vec!["hora", "status", "code", "ms"]).style(theme::accent()))
            .block(Block::bordered().title(Span::styled(" últimas respuestas ", theme::title())));
        f.render_widget(table, body);
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
    use std::time::Instant;

    fn render(screen: &mut CapabilityScreen, ctx: &Ctx) -> String {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| screen.draw(ctx, f, f.area())).unwrap();
        term.backend().to_string()
    }

    fn loaded() -> Event {
        let kind = EventKind::CapabilityLoaded {
            capability: "tool.risk".into(),
            version: "1".into(),
            provider: "toolgate".into(),
            provider_type: "exec".into(),
            flow: None,
            description: "Riesgo de una llamada a herramienta".into(),
            input_fields: vec!["tool_input".into(), "tool_name".into()],
            decision_fields: vec!["reason".into(), "verdict".into()],
        };
        new_event("capability:tool.risk:1", 1, "test", kind)
    }

    fn response(n: u64, status: &str, code: Option<&str>, ms: u64) -> Event {
        let kind = EventKind::HubResponse {
            capability: "tool.risk".into(),
            provider: "toolgate".into(),
            status: status.into(),
            code: code.map(Into::into),
            elapsed_ms: ms,
        };
        let mut e = new_event(&format!("h{n}"), 1, "test", kind);
        e.ts = format!("2026-10-08T09:30:{n:02}Z");
        e
    }

    fn ctx_with_responses(n: u64) -> Ctx {
        let now = Instant::now();
        let mut ctx = Ctx::new(FxMode::Events, now);
        ctx.topo.reduce(&loaded(), now);
        ctx.store.ingest(loaded(), now);
        let script = [("ok", None, 118), ("abstain", None, 96), ("error", Some("deadline_exceeded"), 5000)];
        for i in 0..n {
            let (status, code, ms) = script[i as usize % script.len()];
            let e = response(i, status, code, ms);
            ctx.topo.reduce(&e, now);
            ctx.store.ingest(e, now);
        }
        ctx
    }

    #[test]
    fn capability_card_snapshot() {
        let ctx = ctx_with_responses(5);
        insta::assert_snapshot!(render(&mut CapabilityScreen::new("tool.risk".into()), &ctx));
    }

    #[test]
    fn capability_card_shows_description_and_fields() {
        let out = render(&mut CapabilityScreen::new("tool.risk".into()), &ctx_with_responses(1));
        for needle in ["Riesgo de una llamada", "entrada tool_input, tool_name", "decisión reason, verdict"] {
            assert!(out.contains(needle), "{needle}: {out}");
        }
    }

    #[test]
    fn keeps_only_the_last_twenty_results_newest_first() {
        let ctx = ctx_with_responses(25);
        let times: Vec<String> = ctx.store.hub_results("tool.risk").map(|r| clock(&r.ts)).collect();
        assert_eq!(times.len(), 20);
        assert_eq!(times[0], "09:30:24");
        assert_eq!(times[19], "09:30:05");
    }

    #[test]
    fn unknown_capability_shows_a_notice() {
        let ctx = Ctx::new(FxMode::Events, Instant::now());
        let out = render(&mut CapabilityScreen::new("fantasma".into()), &ctx);
        assert!(out.contains("capacidad no disponible"), "{out}");
    }
}
