//! Subcomando `jevalan lab`.

use crate::{LabAction, LabArgs, load_cfg};
use jevalan_lab::{Filter, Lab, LiveOpts, Mode, ask};
use std::path::PathBuf;
use std::process::ExitCode;

pub fn run(action: LabAction) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `block_on` runs on the main thread (1 MB of stack on Windows, little for TLS and CEL in debug): the work
    // goes to a runtime worker. The stdin confirmation is synchronous and lives inside that task.
    match runtime.block_on(async { tokio::spawn(dispatch(action)).await }) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn dispatch(action: LabAction) -> ExitCode {
    match action {
        LabAction::Check(args) => check(&args),
        LabAction::Offline { capability, fixture, args } => offline(&args, Filter { capability, fixture }).await,
        LabAction::Record { capability, fixture, target, yes, force, args } => {
            let opts = LiveOpts { target, repeat: None, yes, force };
            live(&args, Mode::Record, Filter { capability, fixture }, opts).await
        }
        LabAction::Live { capability, fixture, target, repeat, yes, args } => {
            live(&args, Mode::Live, Filter { capability, fixture }, LiveOpts { target, repeat, yes, force: false })
                .await
        }
    }
}

/// `--config`; otherwise the laboratory's, unless `JEVALAN_CONFIG` (environment or `.env`) already points to one.
fn pick_config(explicit: Option<PathBuf>, env: &dyn Fn(&str) -> Option<String>, own: PathBuf) -> Option<PathBuf> {
    explicit.or_else(|| (env("JEVALAN_CONFIG").is_none() && own.is_file()).then_some(own))
}

fn open(args: &LabArgs) -> Result<Lab, ExitCode> {
    // Without an explicit configuration, the laboratory's own: its backends carry no keys and suffice for offline.
    let dotenv = crate::read_dotenv();
    let env = |key: &str| std::env::var(key).ok().or_else(|| dotenv.get(key).cloned());
    let config = pick_config(args.config.clone(), &env, args.lab_dir.join("config.yaml"));
    let cfg = load_cfg(config.as_deref())?;
    Lab::load(&args.lab_dir, &cfg).map_err(|problems| {
        problems.iter().for_each(|p| eprintln!("{p}"));
        ExitCode::FAILURE
    })
}

fn check(args: &LabArgs) -> ExitCode {
    let lab = match open(args) {
        Ok(lab) => lab,
        Err(code) => return code,
    };
    let problems = lab.check();
    problems.iter().for_each(|p| eprintln!("{p}"));
    lab.notes().iter().for_each(|n| eprintln!("nota: {n}"));
    if problems.is_empty() {
        println!("lab en orden: {} fixtures", lab.fixture_count());
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn offline(args: &LabArgs, filter: Filter) -> ExitCode {
    let lab = match open(args) {
        Ok(lab) => lab,
        Err(code) => return code,
    };
    let problems = lab.check();
    if !problems.is_empty() {
        problems.iter().for_each(|p| eprintln!("{p}"));
        return ExitCode::FAILURE;
    }
    lab.notes().iter().for_each(|n| eprintln!("nota: {n}"));
    let out = lab.offline(&filter).await;
    println!("{}", out.report);
    // A filter that admits nothing, or everything `skipped`, proves nothing: it cannot pass as green.
    let ran = out.results.iter().any(|r| r.skipped.is_none());
    if !ran {
        eprintln!("no se ejecutó ningún fixture");
    }
    match (&out.run_dir, &out.write_error) {
        (Some(dir), _) => println!("resultados en {}", dir.display()),
        (_, Some(e)) => eprintln!("no se pudo escribir en .local/runs/: {e}"),
        _ => {}
    }
    if out.failed || out.write_error.is_some() || !ran { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

async fn live(args: &LabArgs, mode: Mode, filter: Filter, opts: LiveOpts) -> ExitCode {
    let lab = match open(args) {
        Ok(lab) => lab,
        Err(code) => return code,
    };
    let problems = lab.check();
    if !problems.is_empty() {
        problems.iter().for_each(|p| eprintln!("{p}"));
        return ExitCode::FAILURE;
    }
    lab.notes().iter().for_each(|n| eprintln!("nota: {n}"));
    let mut confirm = |message: &str| ask(message, &mut std::io::stdin().lock());
    let out = match lab.live(mode, &filter, &opts, &mut confirm).await {
        Ok(out) => out,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!("{}", out.report);
    match (&out.run_dir, &out.write_error) {
        (Some(dir), _) => println!("resultados en {}", dir.display()),
        (_, Some(e)) => eprintln!("no se pudo escribir: {e}"),
        _ => {}
    }
    // A broken assertion is data in live; the exit fails if the run was cut by budget or could not be written.
    if out.stopped || out.write_error.is_some() { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_reads_jevalan_config_from_dotenv() {
        let own = std::env::current_exe().unwrap();
        let none = |_: &str| None;
        let dotenv = |k: &str| (k == "JEVALAN_CONFIG").then(|| "otra.yaml".to_string());
        assert_eq!(pick_config(None, &none, own.clone()), Some(own.clone()), "sin nada, la del laboratorio");
        assert_eq!(pick_config(None, &dotenv, own.clone()), None, "el .env manda sobre la del laboratorio");
        let explicit = PathBuf::from("x.yaml");
        assert_eq!(pick_config(Some(explicit.clone()), &dotenv, own), Some(explicit), "--config manda siempre");
    }
}
