//! `.local/runs/<run-id>/`: manifiesto, resultados e informe.

use crate::{Lab, ResultLine};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Start instant, already formatted.
pub struct Stamp {
    pub run_id: String,
    pub utc: String,
}

impl Stamp {
    pub fn now() -> Self {
        let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let (y, mo, da, h, mi, s) = civil(d.as_secs());
        let salt = (d.subsec_nanos() ^ std::process::id()) & 0xffff;
        Self {
            run_id: format!("{y:04}{mo:02}{da:02}-{h:02}{mi:02}{s:02}-{salt:04x}"),
            utc: format!("{y:04}-{mo:02}-{da:02}T{h:02}:{mi:02}:{s:02}Z"),
        }
    }
}

/// UTC civil date (Hinnant's algorithm) from some Unix seconds.
fn civil(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// 64-bit FNV-1a in hexadecimal: enough to identify a `lab.yaml`, with no dependencies.
pub fn fnv1a64(text: &str) -> String {
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{hash:016x}")
}

/// Current commit read from `.git` (without launching processes), searching from `from` upwards; `unknown` if there is none.
fn git_commit(from: &std::path::Path) -> String {
    let start = std::path::absolute(from).unwrap_or_else(|_| from.to_path_buf());
    start.ancestors().find_map(|dir| head_of(&dir.join(".git"))).unwrap_or_else(|| "unknown".into())
}

fn head_of(dot_git: &std::path::Path) -> Option<String> {
    // In a worktree `.git` is a file that points to the real directory.
    let git = if dot_git.is_file() {
        let text = std::fs::read_to_string(dot_git).ok()?;
        dot_git.parent()?.join(text.trim().strip_prefix("gitdir:")?.trim())
    } else {
        dot_git.to_path_buf()
    };
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(name) = head.strip_prefix("ref:").map(str::trim) else {
        return is_sha(head).then(|| head.to_string());
    };
    // A worktree's references live in the common directory.
    let common = std::fs::read_to_string(git.join("commondir")).map_or_else(|_| git.clone(), |c| git.join(c.trim()));
    [&git, &common].into_iter().find_map(|dir| {
        let loose = std::fs::read_to_string(dir.join(name)).ok().map(|t| t.trim().to_string());
        let packed = || {
            let text = std::fs::read_to_string(dir.join("packed-refs")).ok()?;
            text.lines().find_map(|l| l.strip_suffix(name)?.trim().split(' ').next().map(String::from))
        };
        loose.or_else(packed).filter(|sha| is_sha(sha))
    })
}

fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What a run with real calls consumed.
#[derive(Default)]
pub struct Consumed {
    pub calls: u64,
    pub cost_usd: f64,
    pub stopped: bool,
    pub repeat: u32,
    /// Model returned by the backend (`body.model`), per target.
    pub reported: BTreeMap<String, String>,
}

pub fn write_run(
    lab: &Lab,
    mode: &str,
    stamp: &Stamp,
    results: &[ResultLine],
    report: &str,
    used: &Consumed,
) -> Result<PathBuf, String> {
    let dir = lab.runs_dir.join(&stamp.run_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let targets: BTreeMap<_, _> = lab
        .config
        .targets
        .iter()
        .map(|(n, t)| {
            let reported = used.reported.get(n).map_or("unknown", String::as_str);
            (n, json!({ "backend": t.backend, "model": t.model, "reported_model": reported }))
        })
        .collect();
    let mut manifest: Value = json!({
        "run_id": stamp.run_id,
        "mode": mode,
        "started_at_utc": stamp.utc,
        "git_commit": git_commit(&lab.root),
        "jevalan_version": env!("CARGO_PKG_VERSION"),
        "lab_yaml_hash": fnv1a64(&lab.yaml),
        "targets": targets,
        "budget": lab.config.budget.map(|b| json!({ "max_calls": b.max_calls, "max_cost_usd": b.max_cost_usd })),
        "calls": used.calls,
        "cost_usd": used.cost_usd,
    });
    if used.repeat > 0 {
        manifest["repeat"] = json!(used.repeat);
    }
    if used.stopped {
        manifest["stopped"] = json!("budget");
    }
    let manifest = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    let lines: Vec<String> = results.iter().filter_map(|r| serde_json::to_string(r).ok()).collect();
    for (name, text) in [
        ("manifest.json", manifest),
        ("results.jsonl", lines.join("\n") + if lines.is_empty() { "" } else { "\n" }),
        ("report.md", report.to_string()),
    ] {
        std::fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(dir)
}

/// Offline report: per capability and target, pass / skipped / fail, and the why of each failure.
pub fn offline_report(results: &[ResultLine]) -> String {
    let mut caps: BTreeMap<&str, Vec<&ResultLine>> = BTreeMap::new();
    for r in results {
        caps.entry(&r.capability).or_default().push(r);
    }
    let mut out = String::from("# Laboratorio · offline\n");
    for (cap, rows) in caps {
        let _ = write!(out, "\n## {cap}\n\n| target | pasa | skipped | fallan |\n|---|---|---|---|\n");
        let targets: std::collections::BTreeSet<&str> = rows.iter().map(|r| r.target.as_str()).collect();
        for t in targets {
            let of = |f: &dyn Fn(&ResultLine) -> bool| rows.iter().filter(|r| r.target == t && f(r)).count();
            let (skipped, passed) = (of(&|r| r.skipped.is_some()), of(&|r| r.passed));
            let failed = of(&|r| r.skipped.is_none() && !r.passed);
            let _ = writeln!(out, "| {t} | {passed} | {skipped} | {failed} |");
        }
        for r in rows.iter().filter(|r| r.skipped.is_none() && !r.passed) {
            let _ = write!(out, "\nFalla `{}` (status {}):\n", r.fixture, r.status);
            for f in &r.failed_asserts {
                match &f.reason {
                    Some(reason) => writeln!(out, "- {} ({reason})", f.why),
                    None => writeln!(out, "- {}", f.why),
                }
                .ok();
            }
        }
        for r in rows.iter().filter(|r| r.skipped.is_some()) {
            let _ = writeln!(out, "\nSkipped `{}`: {}", r.fixture, r.skipped.as_deref().unwrap_or_default());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jevalan-git-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
        dir
    }

    #[test]
    fn reads_a_loose_ref_without_spawning_git() {
        let dir = scratch("loose");
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/master\n").unwrap();
        std::fs::write(dir.join(".git/refs/heads/master"), format!("{SHA}\n")).unwrap();
        std::fs::create_dir_all(dir.join("lab")).unwrap();
        assert_eq!(git_commit(&dir.join("lab")), SHA);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_packed_refs_and_detached_heads() {
        let dir = scratch("packed");
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(dir.join(".git/packed-refs"), format!("# pack-refs\n{SHA} refs/heads/main\n")).unwrap();
        assert_eq!(git_commit(&dir), SHA);
        std::fs::write(dir.join(".git/HEAD"), format!("{SHA}\n")).unwrap();
        assert_eq!(git_commit(&dir), SHA);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_without_a_repository() {
        let dir = std::env::temp_dir().join(format!("jevalan-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The temp dir could hang from a foreign repository: it is enough that it does not return garbage.
        let got = git_commit(&dir);
        assert!(got == "unknown" || is_sha(&got));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
