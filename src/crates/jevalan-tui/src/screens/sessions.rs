//! Sessions screen: the recorded traces of each client, read from the session log.
//! Read-only, loaded when opened and on demand. Note that the tool's input is never shown: screenshots of this
//! view end up on a public blog, and `jevalan sessions show` is the place to read it.

use crate::screen::{Ctx, KeyHint, Msg, Nav, Screen};
use crate::screens::dashboard::footer;
use crate::theme;
use jevalan_hub::sessions::{Session, group, read_dir, timeline, verdict_counts};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEventKind};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use std::path::PathBuf;

const KEYS: &[KeyHint] = &[
    KeyHint { key: "↑↓ jk", desc: "elegir sesión" },
    KeyHint { key: "[ ]", desc: "traza anterior / siguiente" },
    KeyHint { key: "r", desc: "releer el registro" },
    KeyHint { key: "esc", desc: "volver" },
];

/// Minimum width to split list and detail horizontally.
const WIDE: u16 = 100;

pub struct SessionsScreen {
    dir: Option<PathBuf>,
    /// Most recently active first.
    sessions: Vec<Session>,
    selected: usize,
    /// Trace within the selected session; 0 is the most recent.
    trace: usize,
}

impl SessionsScreen {
    pub fn new(dir: Option<PathBuf>) -> Self {
        let mut screen = Self { dir, sessions: Vec::new(), selected: 0, trace: 0 };
        screen.reload();
        screen
    }

    fn reload(&mut self) {
        let Some(dir) = &self.dir else { return };
        let mut sessions = group(read_dir(dir));
        sessions.sort_by(|a, b| last_activity(b).cmp(last_activity(a)));
        self.sessions = sessions;
        self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
        self.trace = self.trace.min(self.current().map_or(0, |s| s.traces.len().saturating_sub(1)));
    }

    fn current(&self) -> Option<&Session> {
        self.sessions.get(self.selected)
    }
}

fn last_activity(session: &Session) -> &str {
    session.traces.last().map_or("", |t| t.end())
}

impl Screen for SessionsScreen {
    fn update(&mut self, _ctx: &mut Ctx, msg: &Msg) -> Option<Nav> {
        let Msg::Key(k) = msg else { return None };
        if k.kind != KeyEventKind::Press {
            return None;
        }
        let last = self.sessions.len().saturating_sub(1);
        let traces = self.current().map_or(0, |s| s.traces.len());
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                self.trace = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(last);
                self.trace = 0;
            }
            KeyCode::Char('[') => self.trace = (self.trace + 1).min(traces.saturating_sub(1)),
            KeyCode::Char(']') => self.trace = self.trace.saturating_sub(1),
            KeyCode::Char('r') => self.reload(),
            _ => {}
        }
        None
    }

    fn draw(&mut self, _ctx: &Ctx, f: &mut Frame, area: Rect) {
        let [head, body, foot] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(5), Constraint::Length(1)]).areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("jevalan", theme::title()),
                Span::styled(" · sesiones capturadas", theme::text()),
            ])),
            head,
        );

        let [list_area, detail_area] = if area.width >= WIDE {
            Layout::horizontal([Constraint::Length(44), Constraint::Fill(1)]).areas(body)
        } else {
            Layout::vertical([Constraint::Min(4), Constraint::Length(12)]).areas(body)
        };

        let block = panel("Sesiones");
        let list_inner = block.inner(list_area);
        f.render_widget(block, list_area);
        if self.sessions.is_empty() {
            let row = Rect::new(
                list_inner.x,
                list_inner.y + list_inner.height / 2,
                list_inner.width,
                1.min(list_inner.height),
            );
            f.render_widget(Paragraph::new(self.empty_message()).style(theme::dim()).alignment(Alignment::Center), row);
        } else {
            let items: Vec<ListItem> = self.sessions.iter().map(|s| ListItem::new(row(s))).collect();
            let list = List::new(items)
                .highlight_symbol("▶ ")
                .highlight_style(Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED));
            let mut state = ListState::default().with_selected(Some(self.selected));
            f.render_stateful_widget(list, list_inner, &mut state);
        }

        let trace = self.current().and_then(|s| s.traces.iter().rev().nth(self.trace).map(|t| (s, t)));
        let title = trace.map_or("Traza".to_string(), |(s, t)| {
            format!("Traza {}/{} · {}", self.trace + 1, s.traces.len(), t.start())
        });
        let block = panel(&title);
        let inner = block.inner(detail_area);
        f.render_widget(block, detail_area);
        if let Some((_, t)) = trace {
            let lines: Vec<Line> = timeline(t, false).into_iter().map(Line::from).collect();
            f.render_widget(Paragraph::new(lines).style(theme::text()).wrap(Wrap { trim: false }), inner);
        }

        f.render_widget(Paragraph::new(footer(KEYS)), foot);
    }

    fn keys(&self) -> &'static [KeyHint] {
        KEYS
    }
}

impl SessionsScreen {
    fn empty_message(&self) -> String {
        match &self.dir {
            None => "no se conoce el directorio de datos · no hay sesiones que leer".into(),
            Some(_) => "sin sesiones · activa `sessions.enabled` en la configuración".into(),
        }
    }
}

fn panel(title: &str) -> Block<'static> {
    Block::bordered().border_style(theme::dim()).title(Span::styled(format!(" {title} "), theme::title()))
}

fn row(s: &Session) -> Line<'static> {
    let counts = verdict_counts(s).into_iter().map(|(word, n)| format!("{word} {n}")).collect::<Vec<_>>().join(" · ");
    Line::from(vec![
        Span::styled(s.client.clone(), theme::text()),
        Span::styled(format!("  {} trazas", s.traces.len()), theme::dim()),
        Span::styled(format!("  {counts}"), theme::dim()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxMode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
    use std::time::Instant;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jevalan-sessions-screen-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn render(screen: &mut SessionsScreen, ctx: &Ctx, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| screen.draw(ctx, f, f.area())).unwrap();
        term.backend().to_string()
    }

    const LOG: &str = concat!(
        r#"{"ts":"2026-10-29T10:00:00Z","trace_id":"t1","client":"claude-code/demo:1","type":"INPUT","capability":"agent.stop","input":{"task":"TOPSECRET"}}"#,
        "\n",
        r#"{"ts":"2026-10-29T10:00:01Z","trace_id":"t1","client":"claude-code/demo:1","type":"DECISION","capability":"agent.stop","status":"ok","decision":{"stop":true,"reason":"verified"}}"#,
        "\n",
        r#"{"ts":"2026-10-29T11:00:00Z","trace_id":"t2","client":"claude-code/demo:1","type":"INPUT","capability":"agent.stop","input":{"task":"TOPSECRET"}}"#,
        "\n",
        r#"{"ts":"2026-10-29T11:00:01Z","trace_id":"t2","client":"claude-code/demo:1","type":"DECISION","capability":"agent.stop","status":"ok","decision":{"stop":false,"reason":"unverified"}}"#,
        "\n",
    );

    #[test]
    fn empty_state_says_how_to_get_sessions() {
        let ctx = Ctx::new(FxMode::Events, Instant::now());
        let dir = temp("empty");
        assert!(render(&mut SessionsScreen::new(Some(dir.clone())), &ctx, 100, 20).contains("sessions.enabled"));
        assert!(render(&mut SessionsScreen::new(None), &ctx, 100, 20).contains("no se conoce el directorio"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn renders_sessions_and_never_the_input() {
        let dir = temp("render");
        std::fs::write(dir.join("log-20261029.jsonl"), LOG).unwrap();
        let ctx = Ctx::new(FxMode::Events, Instant::now());
        let mut screen = SessionsScreen::new(Some(dir.clone()));
        for (w, h) in [(80, 24), (120, 30)] {
            let out = render(&mut screen, &ctx, w, h);
            assert!(!out.contains("TOPSECRET"), "fuga de la entrada en\n{out}");
            for want in ["claude-code/demo:1", "2 trazas", "Traza 1/2", "oculta"] {
                assert!(out.contains(want), "falta {want:?} en\n{out}");
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn brackets_walk_the_traces_and_r_rereads() {
        let dir = temp("keys");
        std::fs::write(dir.join("log-20261029.jsonl"), LOG).unwrap();
        let mut ctx = Ctx::new(FxMode::Events, Instant::now());
        let mut screen = SessionsScreen::new(Some(dir.clone()));
        let press = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));
        screen.update(&mut ctx, &press(KeyCode::Char('[')));
        assert_eq!(screen.trace, 1);
        screen.update(&mut ctx, &press(KeyCode::Char('[')));
        assert_eq!(screen.trace, 1, "no pasa de la traza más antigua");
        screen.update(&mut ctx, &press(KeyCode::Char(']')));
        assert_eq!(screen.trace, 0);
        std::fs::remove_file(dir.join("log-20261029.jsonl")).unwrap();
        screen.update(&mut ctx, &press(KeyCode::Char('r')));
        assert!(screen.sessions.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
