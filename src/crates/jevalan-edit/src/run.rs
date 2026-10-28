//! Bucle de terminal del editor. Copia el patrón de `jevalan_tui::run`, que está atado al dashboard.

use crate::app::App;
use ratatui::crossterm::event::{self, Event as TermEvent};
use std::io;
use std::time::Duration;

/// Bucle de terminal: lee el teclado, late con el bus y pinta solo cuando hace falta.
pub fn run(mut app: App) -> io::Result<()> {
    // `ratatui::init` deja puesto el hook que restaura el terminal si algo revienta; el editor no
    // captura el ratón (D4), así que no hay nada más que soltar al salir.
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
                    // El aviso de «terminal demasiado pequeño» sale de comparar el área al pintar.
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
