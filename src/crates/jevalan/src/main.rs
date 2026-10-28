mod edit;
mod fixtures;
mod hook;
mod lab;
mod live;
mod shadow;
mod specialist;

use clap::{Parser, Subcommand};
use jevalan_daemon::config::{load, parse_dotenv};
use jevalan_tui::fx::FxMode;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "jevalan", version, about = "Hub local de decisiones")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Dashboard de terminal.
    Tui {
        /// Alimenta la TUI con trazas ficticias en vez de conectarse al daemon.
        #[arg(long, conflicts_with = "demo")]
        fake: bool,
        /// WebSocket de eventos del daemon.
        #[arg(long, default_value = "ws://127.0.0.1:8080/events")]
        url: String,
        /// Token del daemon, si exige autenticación.
        #[arg(long, env = "JEVALAN_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Efectos visuales: off, events o full.
        #[arg(long, env = "JEVALAN_FX", default_value = "events")]
        fx: FxMode,
        /// Peticiones ficticias por segundo.
        #[arg(long, default_value_t = 2.0)]
        fake_rate: f64,
        /// Semilla del generador; sin ella, cada arranque es distinto.
        #[arg(long)]
        fake_seed: Option<u64>,
        /// Fichero de configuración del daemon propio, si hay que convocarlo (sin él, como `serve`).
        #[arg(long)]
        config: Option<PathBuf>,
        /// Demostración: backends simulados si hay que convocar daemon, y tráfico incesante.
        #[arg(long)]
        demo: bool,
        /// Peticiones por segundo del tráfico de `--demo`.
        #[arg(long, default_value_t = 2.0)]
        rate: f64,
    },
    /// Daemon: proxy de System One, flujos de decisión y bus de eventos.
    Serve {
        /// Fichero de configuración; sin él, `JEVALAN_CONFIG` o los valores por defecto.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Editor visual de flujos: abre un `config/flows/*.yaml` y lo edita sin publicarlo.
    Edit {
        /// Fichero del flujo que se abre.
        file: PathBuf,
        /// Dirección del daemon; sin ella, la de `daemon.json`, y sin ella ninguna.
        #[arg(long)]
        url: Option<String>,
        /// Token del daemon, si exige autenticación.
        #[arg(long, env = "JEVALAN_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// No escuchar el bus de eventos (solo se pierde la telemetría).
        #[arg(long)]
        no_events: bool,
        /// Catálogo de señales con que se valida en local.
        #[arg(long, default_value = "./config/signals")]
        signals_dir: PathBuf,
    },
    /// Cliente de agentes externos.
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Laboratorio: comprueba, reproduce, graba y mide fixtures.
    Lab {
        #[command(subcommand)]
        action: LabAction,
    },
    /// Registro sombra del hook: informes sobre `shadow.jsonl`.
    Shadow {
        #[command(subcommand)]
        action: ShadowAction,
    },
    /// Especialista simulado (lo usa la demo como proveedor `exec`).
    #[command(hide = true)]
    MockSpecialist {
        capability: String,
        /// Responde siempre `fail-mode`.
        #[arg(long)]
        fail: bool,
        /// Falla mientras exista este fichero.
        #[arg(long)]
        fail_flag: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum HookAction {
    /// Hook `PreToolUse` genérico: lee el JSON de stdin y consulta `tool.risk`.
    Run {
        /// Nombre con que el daemon reconoce la sesión (p. ej. `crush`).
        #[arg(long, default_value = "unknown")]
        client: String,
        /// Dialecto de salida que entiende el harness.
        #[arg(long, value_enum, default_value_t = hook::Dialect::Claude)]
        dialect: hook::Dialect,
        /// shadow: solo registra y calla; enforce: puede responder deny o ask.
        #[arg(long, value_enum, default_value_t = hook::Mode::Shadow)]
        mode: hook::Mode,
    },
    /// Alias de `run --client claude-code --dialect claude`.
    ClaudeCode {
        /// shadow: solo registra y calla; enforce: puede responder deny o ask.
        #[arg(long, value_enum, default_value_t = hook::Mode::Shadow)]
        mode: hook::Mode,
    },
    /// Alias de `run --client crush --dialect crush`.
    Crush {
        /// shadow: solo registra y calla; enforce: puede bloquear con deny.
        #[arg(long, value_enum, default_value_t = hook::Mode::Shadow)]
        mode: hook::Mode,
    },
}

#[derive(Subcommand)]
enum ShadowAction {
    /// Informe Markdown del registro: veredictos, anatomía del ask, probabilidades y latencias.
    Report {
        /// Registro a leer; por defecto, `shadow.jsonl` del directorio de datos.
        #[arg(long)]
        log: Option<PathBuf>,
        /// Solo desde esta fecha (RFC 3339 o AAAA-MM-DD, en UTC).
        #[arg(long)]
        since: Option<String>,
        /// Fichero de salida; sin él, la salida estándar.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Incluye la entrada (redactada, truncada) de los ejemplos; solo para uso local.
        #[arg(long)]
        with_inputs: bool,
    },
    /// Convierte decisiones reales del registro en fixtures del laboratorio.
    Fixtures {
        /// Registro a leer; por defecto, `shadow.jsonl` del directorio de datos.
        #[arg(long)]
        log: Option<PathBuf>,
        /// Solo desde esta fecha (RFC 3339 o AAAA-MM-DD, en UTC).
        #[arg(long)]
        since: Option<String>,
        /// Banco de fixtures; se crea si no existe, con su `lab.yaml`.
        #[arg(long, default_value = "lab-fixtures-batch")]
        lab_dir: PathBuf,
        /// Solo los N casos de mayor riesgo primero; el resto se cuenta como recortado.
        #[arg(long)]
        max: Option<usize>,
    },
}

#[derive(Subcommand)]
enum LabAction {
    /// Valida `lab.yaml` y los fixtures, sin red.
    Check(LabArgs),
    /// Reproduce los fixtures con su `replay` o con las cassettes.
    Offline {
        /// Solo esta capacidad.
        capability: Option<String>,
        /// Solo este fixture.
        #[arg(long)]
        fixture: Option<String>,
        #[command(flatten)]
        args: LabArgs,
    },
    /// Llama a los backends reales y graba cassettes (con presupuesto).
    Record {
        /// Solo esta capacidad.
        capability: Option<String>,
        /// Solo este fixture.
        #[arg(long)]
        fixture: Option<String>,
        /// Solo este target.
        #[arg(long)]
        target: Option<String>,
        /// No pedir confirmación.
        #[arg(long)]
        yes: bool,
        /// Sobrescribe la cassette aunque la grabación traiga fallos (401, 500, error).
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        args: LabArgs,
    },
    /// Fixtures × targets × repeticiones contra backends reales (con presupuesto).
    Live {
        /// Solo esta capacidad.
        capability: Option<String>,
        /// Solo este fixture.
        #[arg(long)]
        fixture: Option<String>,
        /// Solo este target.
        #[arg(long)]
        target: Option<String>,
        /// Repeticiones; sustituye al `repeat` de lab.yaml.
        #[arg(long)]
        repeat: Option<u32>,
        /// No pedir confirmación.
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        args: LabArgs,
    },
}

#[derive(clap::Args)]
struct LabArgs {
    /// Fichero de configuración del daemon; sin él, `JEVALAN_CONFIG`, el `config.yaml` del laboratorio o los valores por defecto.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Directorio del laboratorio.
    #[arg(long, default_value = "config/lab")]
    lab_dir: PathBuf,
}

fn main() -> ExitCode {
    match Cli::parse().cmd {
        Cmd::Tui { fake, url, token, fx, fake_rate, fake_seed, config, demo, rate } => {
            if fake {
                tui_fake(fx, fake_rate, fake_seed)
            } else {
                live::run(live::Opts { url, token, fx, config, demo, rate })
            }
        }
        Cmd::Serve { config } => serve(config),
        Cmd::Edit { file, url, token, no_events, signals_dir } => {
            edit::run(edit::Opts { file, url, token, no_events, signals_dir })
        }
        Cmd::Hook { action } => match action {
            HookAction::Run { client, dialect, mode } => hook::run(mode, &client, dialect),
            HookAction::ClaudeCode { mode } => hook::run(mode, "claude-code", hook::Dialect::Claude),
            HookAction::Crush { mode } => hook::run(mode, "crush", hook::Dialect::Crush),
        },
        Cmd::Lab { action } => lab::run(action),
        Cmd::Shadow { action } => match action {
            ShadowAction::Report { log, since, out, with_inputs } => {
                shadow::run(shadow::Opts { log, since, out, with_inputs })
            }
            ShadowAction::Fixtures { log, since, lab_dir, max } => fixtures::run(log, since, lab_dir, max),
        },
        Cmd::MockSpecialist { capability, fail, fail_flag } => specialist::run(&capability, fail, fail_flag.as_deref()),
    }
}

/// `.env` del directorio actual, si existe; vacío si no.
fn read_dotenv() -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(".env").map(|text| parse_dotenv(&text)).unwrap_or_default()
}

fn tui_fake(fx: FxMode, fake_rate: f64, fake_seed: Option<u64>) -> ExitCode {
    let seed = fake_seed.unwrap_or_else(now_seed);
    finish(jevalan_tui::run::run(
        jevalan_tui::fake::spawn(fake_rate, seed),
        fx,
        "fake",
        None,
        crate::shadow::default_log(),
    ))
}

fn now_seed() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
}

fn finish(result: std::io::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Configuración del daemon como la arma `serve`: fichero, `JEVALAN_*` y `.env`.
fn load_cfg(config: Option<&std::path::Path>) -> Result<jevalan_daemon::config::Config, ExitCode> {
    let dotenv = read_dotenv();
    let env = |key: &str| std::env::var(key).ok().or_else(|| dotenv.get(key).cloned());
    match load(config, &env) {
        Ok(mut cfg) => {
            cfg.hub.dotenv = dotenv.clone();
            Ok(cfg)
        }
        Err(e) => {
            eprintln!("configuración inválida: {e}");
            Err(ExitCode::from(2))
        }
    }
}

fn serve(config: Option<PathBuf>) -> ExitCode {
    let cfg = match load_cfg(config.as_deref()) {
        Ok(cfg) => cfg,
        Err(code) => return code,
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `block_on` ejecuta el futuro en el hilo principal (1 MB de pila en Windows): el arranque, que compila CEL
    // al cargar flujos y capacidades, se lanza en un worker del runtime.
    let result = runtime.block_on(async { tokio::spawn(jevalan_daemon::serve(cfg)).await });
    match result {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
