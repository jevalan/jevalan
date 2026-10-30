//! Terminal loop: reads keyboard, mouse and events, and paints only when needed.

use crate::app::App;
use crate::fx::FxMode;
use crate::screen::{BackendToggle, Feed, Msg};
use crate::screens::dashboard::Dashboard;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent};
use ratatui::crossterm::execute;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

pub fn run(
    rx: Receiver<Feed>,
    fx: FxMode,
    source: &'static str,
    toggle: Option<BackendToggle>,
    shadow_log: Option<PathBuf>,
    sessions_dir: Option<PathBuf>,
) -> io::Result<()> {
    // ratatui::init chains this hook: it first restores the terminal and then releases the mouse.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        previous(info);
    }));
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;
    let result = event_loop(&mut terminal, &rx, fx, source, toggle, shadow_log, sessions_dir);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    rx: &Receiver<Feed>,
    fx: FxMode,
    source: &'static str,
    toggle: Option<BackendToggle>,
    shadow_log: Option<PathBuf>,
    sessions_dir: Option<PathBuf>,
) -> io::Result<()> {
    let mut app = App::new(Box::new(Dashboard::new()), fx, Instant::now());
    app.ctx.source = source;
    app.ctx.toggle = toggle;
    app.ctx.shadow_log = shadow_log;
    app.ctx.sessions_dir = sessions_dir;
    loop {
        if app.needs_redraw() {
            terminal.draw(|f| app.draw(f))?;
        }
        if event::poll(Duration::from_millis(16))? {
            loop {
                match event::read()? {
                    TermEvent::Key(k) => app.update(Msg::Key(k)),
                    TermEvent::Mouse(m) => app.update(Msg::Mouse(m)),
                    TermEvent::Resize(w, h) => app.update(Msg::Resize(w, h)),
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        app.update(Msg::Tick(Instant::now()));
        loop {
            match rx.try_recv() {
                Ok(Feed::Event(e)) => app.update(Msg::Event(Box::new(e))),
                Ok(Feed::Status(s)) => app.update(Msg::Status(s)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    app.mark_source_closed();
                    break;
                }
            }
        }
        if app.quit {
            return Ok(());
        }
    }
}
