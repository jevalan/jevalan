//! `jevalan edit`: opens a flow in the visual editor, without publishing it.
//!
//! The subcommand publishes nothing: it builds the document, offers the draft the daemon that
//! validates (if known) and the event bus telemetry (if wanted), and hands the
//! terminal over to the workbench. The file is the only thing written, and only when the
//! user presses `s`.

use jevalan_daemon::discovery;
use jevalan_edit::app::App;
use jevalan_edit::daemon::Cliente;
use jevalan_edit::documento::Documento;
use jevalan_edit::edicion::Borrador;
use jevalan_edit::telemetria::Telemetria;
use std::path::PathBuf;
use std::process::ExitCode;

/// What the CLI hands to the editor.
pub struct Opts {
    pub file: PathBuf,
    pub url: Option<String>,
    pub token: Option<String>,
    pub no_events: bool,
    pub signals_dir: PathBuf,
}

/// Opens the editor; returns the process exit code.
pub fn run(opts: Opts) -> ExitCode {
    // Without `--url`, the daemon is looked up the way the Claude Code hook does it: the profile's `daemon.json`.
    let (base, token) = match &opts.url {
        Some(url) => (url.clone(), opts.token.clone()),
        None => match discovery::read() {
            Some((addr, token)) => (format!("http://{addr}"), opts.token.clone().or(token)),
            None => (String::new(), opts.token.clone()),
        },
    };

    let documento = match Documento::abrir(&opts.file) {
        Ok(documento) => documento,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let nombre = documento.nombre();
    let mut borrador = Borrador::nuevo(documento);
    // Without a daemon, the local parser needs the catalogue: a flow with signals cannot be judged without it.
    borrador.senales(jevalan_daemon::signals::load_dir(&opts.signals_dir).0);
    // The bottom band must say why validation happens locally instead of against the daemon.
    let mut aviso = None;
    if base.is_empty() {
        aviso = Some("sin daemon conocido: se valida con el parser local".to_string());
    } else {
        match Cliente::nuevo(&base, token.clone()) {
            Ok(cliente) => borrador.con_daemon(Some(cliente)),
            Err(e) => aviso = Some(e),
        }
    }
    let telemetria = if opts.no_events || base.is_empty() {
        Telemetria::apagada(&nombre)
    } else {
        let ws = format!("{}/events", base.replacen("http://", "ws://", 1));
        Telemetria::conectar(&ws, token.as_deref(), &nombre)
    };

    let mut app = App::nuevo(borrador, telemetria);
    app.aviso_banda = aviso;
    match jevalan_edit::run(app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
