//! `jevalan tui` contra un daemon: lo usa si ya late, lo convoca en el propio proceso si no.

use crate::{finish, now_seed, read_dotenv};
use jevalan_daemon::config::{Config, load};
use jevalan_daemon::discovery;
use jevalan_daemon::mock::{Switches, demo_config};
use jevalan_daemon::probe::{Probe, probe, wait_healthy};
use jevalan_daemon::simulate::spawn_simulator;
use jevalan_tui::fx::FxMode;
use jevalan_tui::screen::BackendToggle;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const STARTUP_WAIT: Duration = Duration::from_secs(5);
/// Dirección predeterminada de `jevalan tui` (la misma que declara su `--url`).
const DEFAULT_URL: &str = "ws://127.0.0.1:8080/events";

pub struct Opts {
    pub url: String,
    pub token: Option<String>,
    pub fx: FxMode,
    pub config: Option<PathBuf>,
    pub demo: bool,
    pub rate: f64,
}

pub fn run(opts: Opts) -> ExitCode {
    // Todo lo previo a la terminal: ahí aún se puede protestar por stderr con decoro.
    let Some(base) = http_base(&opts.url) else {
        eprintln!("error: --url debe ser ws:// o wss:// con host y puerto, no «{}»", opts.url);
        return ExitCode::from(2);
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return fail(&e.to_string()),
    };
    // El entorno ya lo lee clap; el `.env` solo rellena el hueco.
    let dotenv = read_dotenv();
    let mut switches = Switches::new();
    let mut token = opts.token.or_else(|| dotenv.get("JEVALAN_TOKEN").cloned());

    match runtime.block_on(probe(&base)) {
        // El fichero de descubrimiento describe al daemon en ejecución; un daemon ausente deja un fichero caduco.
        Probe::Jevalan => token = token.or_else(|| discovered_token(&base, &opts.url)),
        Probe::Foreign => {
            eprintln!("error: en {base} escucha algo que no es jevalan; use otro --url o libere el puerto");
            return ExitCode::from(2);
        }
        Probe::Down => {
            let cfg = match embedded_config(&runtime, &base, &opts.config, opts.demo, &dotenv) {
                Ok((cfg, s)) => {
                    switches = s;
                    cfg
                }
                Err((code, msg)) => {
                    eprintln!("{msg}");
                    return code;
                }
            };
            token = token.or_else(|| cfg.token.clone());
            let server = runtime.spawn(jevalan_daemon::serve_quiet(cfg, std::future::pending()));
            if let Err(msg) = runtime.block_on(await_daemon(&base, &server)) {
                return fail(&msg);
            }
        }
    }
    if opts.demo {
        let _guard = runtime.enter();
        spawn_simulator(base, token.clone(), opts.rate, now_seed());
    }
    // Al volver de aquí caen el runtime, el daemon propio, los mocks y el simulador.
    // Solo el daemon propio de la demo tiene mocks que derribar.
    let toggle: Option<BackendToggle> = (!switches.is_empty())
        .then(|| Box::new(move |name: &str| switches.get(name).map(|switch| switch.toggle())) as BackendToggle);
    finish(jevalan_tui::run::run(
        jevalan_tui::live::spawn(opts.url, token),
        opts.fx,
        "live",
        toggle,
        crate::shadow::default_log(),
    ))
}

/// Token de `daemon.json`, solo si ese fichero describe el daemon de `base` o si `url` es el predeterminado.
/// Así no se envía el token a una dirección ajena a la que el fichero describe.
fn discovered_token(base: &str, url: &str) -> Option<String> {
    let (addr, token) = discovery::read()?;
    let host = base.split("://").nth(1).unwrap_or_default();
    if url == DEFAULT_URL || addr == host { token } else { None }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::FAILURE
}

/// `ws://host:port/events` -> `http://host:port`.
fn http_base(url: &str) -> Option<String> {
    let (scheme, rest) = match (url.strip_prefix("ws://"), url.strip_prefix("wss://")) {
        (Some(rest), _) => ("http", rest),
        (_, Some(rest)) => ("https", rest),
        _ => return None,
    };
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(format!("{scheme}://{host}"))
}

type Failure = (ExitCode, String);

fn embedded_config(
    runtime: &tokio::runtime::Runtime,
    base: &str,
    config: &Option<PathBuf>,
    demo: bool,
    dotenv: &std::collections::BTreeMap<String, String>,
) -> Result<(Config, Switches), Failure> {
    let invalid = |m: String| (ExitCode::from(2), m);
    let host = base.split("://").nth(1).unwrap_or_default();
    let addr: SocketAddr = host
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .ok_or_else(|| invalid(format!("error: no se entiende la dirección {host}")))?;
    if demo {
        let fail = |e: std::io::Error| (ExitCode::FAILURE, format!("error: {e}"));
        let exe = std::env::current_exe().map_err(fail)?;
        let specialist = vec![exe.display().to_string(), "mock-specialist".into(), "tool.risk".into()];
        return runtime.block_on(demo_config(addr, now_seed(), specialist)).map_err(fail);
    }
    let env = |key: &str| std::env::var(key).ok().or_else(|| dotenv.get(key).cloned());
    let mut cfg = load(config.as_deref(), &env).map_err(|e| invalid(format!("configuración inválida: {e}")))?;
    cfg.addr = addr;
    cfg.hub.dotenv = dotenv.clone();
    Ok((cfg, Switches::new()))
}

/// Espera a que el daemon propio conteste, o a que muera diciendo por qué.
async fn await_daemon(base: &str, server: &tokio::task::JoinHandle<std::io::Result<()>>) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + STARTUP_WAIT;
    while !server.is_finished() && tokio::time::Instant::now() < deadline {
        if wait_healthy(base, Duration::from_millis(100)).await {
            return Ok(());
        }
    }
    if server.is_finished() {
        return Err("el daemon propio se detuvo nada más nacer (¿puerto ocupado?)".into());
    }
    Err(format!("el daemon propio no contesta en {base}"))
}

#[cfg(test)]
mod tests {
    use super::http_base;

    #[test]
    fn ws_url_becomes_http_base() {
        assert_eq!(http_base("ws://127.0.0.1:8080/events").as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(http_base("wss://h:1").as_deref(), Some("https://h:1"));
        assert_eq!(http_base("http://h:1/events"), None);
        assert_eq!(http_base("ws:///events"), None);
    }
}
