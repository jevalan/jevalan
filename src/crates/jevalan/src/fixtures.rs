//! Subcomando `jevalan shadow fixtures`: convierte decisiones reales del registro en fixtures del
//! laboratorio. Cabe señalar que la lectura del registro es la misma que la del informe.

use crate::shadow::{default_log, since_filter};
use jevalan_hub::shadow::parse;
use jevalan_lab::import::{self, Opts};
use std::path::PathBuf;
use std::process::ExitCode;

pub fn run(log: Option<PathBuf>, since: Option<String>, lab_dir: PathBuf, max: Option<usize>) -> ExitCode {
    let Some(path) = log.or_else(default_log) else {
        eprintln!("error: no se encuentra el directorio de datos; indique --log");
        return ExitCode::FAILURE;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: no se puede leer {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let (mut entries, ignored) = parse(&text);
    if let Some(since) = &since {
        entries = since_filter(entries, since);
    }
    println!("Registro: {} — {} entradas, {ignored} líneas ignoradas", path.display(), entries.len());
    match import::import(&entries, ignored, &Opts { lab_dir, max }) {
        Ok(tally) => {
            println!("{}", tally.render());
            if tally.no_room > 0 {
                eprintln!("error: {} casos se quedaron sin id libre", tally.no_room);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
