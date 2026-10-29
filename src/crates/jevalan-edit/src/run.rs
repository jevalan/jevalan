//! The editor's terminal loop. Copies the pattern of `jevalan_tui::run`, which is tied to the dashboard.

use crate::app::App;
use ratatui::crossterm::event::{self, Event as TermEvent};
use std::io;
use std::time::Duration;

/// Terminal loop: reads the keyboard, beats with the bus and paints only when needed.
pub fn run(mut app: App) -> io::Result<()> {
    // `ratatui::init` installs the hook that restores the terminal if something blows up; the editor does not
    // capture the mouse (D4), so there is nothing more to release on exit.
    let mut terminal = ratatui::init();
    let resultado = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    resultado
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    loop {
        if app.redibujar {
            terminal.draw(|f| app.draw(f))?;
        }
        if event::poll(Duration::from_millis(16))? {
            loop {
                match event::read()? {
                    TermEvent::Key(k) => app.tecla(k),
                    // The "terminal too small" notice comes from comparing the area when painting.
                    TermEvent::Resize(_, _) => app.redibujar = true,
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        app.latir();
        if app.salir {
            return Ok(());
        }
    }
}
