mod edit;
mod fixtures;
mod hook;
mod lab;
mod live;
mod mcp;
mod sessions;
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
    /// Terminal dashboard.
    Tui {
        /// Feeds the TUI with fictitious traces instead of connecting to the daemon.
        #[arg(long, conflicts_with = "demo")]
        fake: bool,
        /// The daemon's event WebSocket.
        #[arg(long, default_value = "ws://127.0.0.1:8080/events")]
        url: String,
        /// Daemon token, if it requires authentication.
        #[arg(long, env = "JEVALAN_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Efectos visuales: off, events o full.
        #[arg(long, env = "JEVALAN_FX", default_value = "events")]
        fx: FxMode,
        /// Peticiones ficticias por segundo.
        #[arg(long, default_value_t = 2.0)]
        fake_rate: f64,
        /// Generator seed; without it, every start is different.
        #[arg(long)]
        fake_seed: Option<u64>,
        /// Configuration file of the own daemon, if one has to be summoned (without it, as `serve`).
        #[arg(long)]
        config: Option<PathBuf>,
        /// Demonstration: simulated backends if a daemon has to be summoned, and incessant traffic.
        #[arg(long)]
        demo: bool,
        /// Requests per second of the `--demo` traffic.
        #[arg(long, default_value_t = 2.0)]
        rate: f64,
    },
    /// Daemon: System One proxy, decision flows and event bus.
    Serve {
        /// Configuration file; without it, `JEVALAN_CONFIG` or the defaults.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Visual flow editor: opens a `config/flows/*.yaml` and edits it without publishing it.
    Edit {
        /// File of the flow to open.
        file: PathBuf,
        /// Daemon address; without it, the one in `daemon.json`, and without that, none.
        #[arg(long)]
        url: Option<String>,
        /// Daemon token, if it requires authentication.
        #[arg(long, env = "JEVALAN_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Do not listen to the event bus (only the telemetry is lost).
        #[arg(long)]
        no_events: bool,
        /// Signal catalogue used for local validation.
        #[arg(long, default_value = "./config/signals")]
        signals_dir: PathBuf,
    },
    /// MCP server over stdio: exposes the hub's capabilities as tools.
    Mcp {
        /// Daemon `host:port` address; without it, the one in `daemon.json`, and then the default.
        #[arg(long)]
        addr: Option<String>,
        /// Daemon token, if it requires authentication.
        #[arg(long, env = "JEVALAN_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Lets `jevalan_try_draft` use the `live` and `record` transports, which call real models.
        #[arg(long)]
        allow_live: bool,
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
    /// The hook's shadow log: reports over `shadow.jsonl`.
    Shadow {
        #[command(subcommand)]
        action: ShadowAction,
    },
    /// The daemon's session log (opt-in: `sessions.enabled`): what was asked, which backends answered, what was decided.
    Sessions {
        /// Log directory; by default `<data>/sessions`.
        #[arg(long, global = true)]
        dir: Option<PathBuf>,
        #[command(subcommand)]
        action: SessionsAction,
    },
    /// Simulated specialist (the demo uses it as an `exec` provider).
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
    /// Generic `PreToolUse` hook: reads the JSON on stdin and queries `tool.risk`.
    Run {
        /// Name under which the daemon recognises the session (e.g. `crush`).
        #[arg(long, default_value = "unknown")]
        client: String,
        /// Output dialect the harness understands.
        #[arg(long, value_enum, default_value_t = hook::Dialect::Claude)]
        dialect: hook::Dialect,
        /// shadow: only records and stays silent; enforce: may answer deny or ask.
        #[arg(long, value_enum, default_value_t = hook::Mode::Shadow)]
        mode: hook::Mode,
    },
    /// Alias de `run --client claude-code --dialect claude`.
    ClaudeCode {
        /// shadow: only records and stays silent; enforce: may answer deny or ask.
        #[arg(long, value_enum, default_value_t = hook::Mode::Shadow)]
        mode: hook::Mode,
    },
    /// Alias de `run --client crush --dialect crush`.
    Crush {
        /// shadow: only records and stays silent; enforce: may block with deny.
        #[arg(long, value_enum, default_value_t = hook::Mode::Shadow)]
        mode: hook::Mode,
    },
}

#[derive(Subcommand)]
enum SessionsAction {
    /// One row per client: first and last activity, traces and decisions by verdict.
    List {
        /// Only traces since this date (RFC 3339 or YYYY-MM-DD, in UTC).
        #[arg(long, value_parser = shadow::parse_since)]
        since: Option<String>,
        /// Only clients whose name contains this text.
        #[arg(long)]
        client: Option<String>,
        /// JSON instead of text, to chain with `jq`.
        #[arg(long)]
        json: bool,
    },
    /// Timeline of a client's traces, or of one trace: input, path, backends and decision.
    Show {
        /// The client (`crush/crush:243a`) or a `trace_id`.
        target: String,
        /// JSON instead of text, to chain with `jq`.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ShadowAction {
    /// Markdown report of the log: verdicts, anatomy of the ask, probabilities and latencies.
    Report {
        /// Log to read; by default, `shadow.jsonl` in the data directory.
        #[arg(long)]
        log: Option<PathBuf>,
        /// Only since this date (RFC 3339 or YYYY-MM-DD, in UTC).
        #[arg(long, value_parser = shadow::parse_since)]
        since: Option<String>,
        /// Output file; without it, standard output.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Includes the (redacted, truncated) input of the examples; for local use only.
        #[arg(long)]
        with_inputs: bool,
    },
    /// Turns real decisions from the log into laboratory fixtures.
    Fixtures {
        /// Log to read; by default, `shadow.jsonl` in the data directory.
        #[arg(long)]
        log: Option<PathBuf>,
        /// Only since this date (RFC 3339 or YYYY-MM-DD, in UTC).
        #[arg(long, value_parser = shadow::parse_since)]
        since: Option<String>,
        /// Fixture bench; created if it does not exist, with its `lab.yaml`.
        #[arg(long, default_value = "lab-fixtures-batch")]
        lab_dir: PathBuf,
        /// Only the N highest-risk cases first; the rest are counted as trimmed.
        #[arg(long)]
        max: Option<usize>,
    },
}

#[derive(Subcommand)]
enum LabAction {
    /// Validates `lab.yaml` and the fixtures, with no network.
    Check(LabArgs),
    /// Replays the fixtures with their `replay` or with the cassettes.
    Offline {
        /// Only this capability.
        capability: Option<String>,
        /// Only this fixture.
        #[arg(long)]
        fixture: Option<String>,
        #[command(flatten)]
        args: LabArgs,
    },
    /// Calls the real backends and records cassettes (with a budget).
    Record {
        /// Only this capability.
        capability: Option<String>,
        /// Only this fixture.
        #[arg(long)]
        fixture: Option<String>,
        /// Only this target.
        #[arg(long)]
        target: Option<String>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Overwrites the cassette even if the recording contains failures (401, 500, error).
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        args: LabArgs,
    },
    /// Fixtures × targets × repeticiones contra backends reales (con presupuesto).
    Live {
        /// Only this capability.
        capability: Option<String>,
        /// Only this fixture.
        #[arg(long)]
        fixture: Option<String>,
        /// Only this target.
        #[arg(long)]
        target: Option<String>,
        /// Repetitions; overrides the `repeat` of lab.yaml.
        #[arg(long)]
        repeat: Option<u32>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        args: LabArgs,
    },
}

#[derive(clap::Args)]
struct LabArgs {
    /// Daemon configuration file; without it, `JEVALAN_CONFIG`, the laboratory's `config.yaml` or the defaults.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Laboratory directory.
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
        Cmd::Mcp { addr, token, allow_live } => mcp::run(mcp::McpOptions { addr, token, allow_live }),
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
        Cmd::Sessions { dir, action } => sessions::run(sessions::Opts {
            dir,
            action: match action {
                SessionsAction::List { since, client, json } => sessions::Action::List { since, client, json },
                SessionsAction::Show { target, json } => sessions::Action::Show { target, json },
            },
        }),
        Cmd::MockSpecialist { capability, fail, fail_flag } => specialist::run(&capability, fail, fail_flag.as_deref()),
    }
}

/// `.env` of the current directory, if it exists; empty if not.
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

/// Daemon configuration as `serve` builds it: file, `JEVALAN_*` and `.env`.
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
    // `block_on` runs the future on the main thread (1 MB of stack on Windows): the start-up, which compiles CEL
    // when loading flows and capabilities, is launched on a runtime worker.
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
