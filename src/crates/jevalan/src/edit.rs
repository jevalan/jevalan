//! `jevalan edit`: abre un flujo con el editor visual, sin publicarlo.
//!
//! El subcomando no publica nada: monta el documento, le ofrece al borrador el daemon que
//! valida (si se conoce) y la telemetría del bus de eventos (si se quiere), y cede el
//! terminal al banco de trabajo. El fichero es lo único que se escribe, y solo cuando el
//! usuario pulsa `s`.

use jevalan_daemon::discovery;
use jevalan_edit::app::App;
use jevalan_edit::daemon::Cliente;
use jevalan_edit::documento::Documento;
use jevalan_edit::edicion::Borrador;
use jevalan_edit::telemetria::Telemetria;
use std::path::PathBuf;
use std::process::ExitCode;

/// Lo que la CLI le pasa al editor.
pub struct Opts {
    pub file: PathBuf,
    pub url: Option<String>,
    pub token: Option<String>,
    pub no_events: bool,
    pub signals_dir: PathBuf,
}

/// Abre el editor; devuelve el código de salida del proceso.
pub fn run(opts: Opts) -> ExitCode {
    // Sin `--url`, se busca el daemon como lo hace el hook de Claude Code: `daemon.json` del perfil.
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
    // Sin daemon, el parser local necesita el catálogo: un flujo con señales no se juzga sin él.
    borrador.senales(jevalan_daemon::signals::load_dir(&opts.signals_dir).0);
    // La banda inferior ha de contar por qué se valida en local en vez de contra el daemon.
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
