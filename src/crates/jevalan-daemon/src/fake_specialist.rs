//! Stand-in specialist for the `exec` tests: answers according to `input.mode`.

use std::io::{Read, Write};

fn main() {
    let mut text = String::new();
    let _ = std::io::stdin().read_to_string(&mut text);
    let input: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let verdict = |v: &str, reason: &str, source: &str| {
        println!("{}", serde_json::json!({ "verdict": v, "reason": reason, "source": source }));
    };
    match input["mode"].as_str().unwrap_or_default() {
        "none" => verdict("passthrough", "n/a", "no-opinion"),
        "fail" => verdict("ask", "down", "fail-mode"),
        "maybe" => verdict("maybe", "dudoso", "model"),
        "sleep" => std::thread::sleep(std::time::Duration::from_secs(10)),
        "spawn-grandchild" => {
            // A long-lived grandchild that inherits stdout; its PID is left in `input.pid_file`.
            let exe = std::env::current_exe().expect("current_exe");
            #[allow(clippy::zombie_processes)] // the grandchild must outlive its parent
            let mut child =
                std::process::Command::new(exe).stdin(std::process::Stdio::piped()).spawn().expect("grandchild");
            let _ = child.stdin.take().expect("stdin").write_all(br#"{"mode":"sleep"}"#);
            if let Some(path) = input["pid_file"].as_str() {
                std::fs::write(path, child.id().to_string()).expect("pid_file");
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
        "flood" => {
            let block = [b'x'; 64 * 1024];
            let mut out = std::io::stdout();
            for _ in 0..32 {
                let _ = out.write_all(&block);
            }
        }
        "garbage" => println!("no json"),
        "exit" => {
            eprintln!("fallo simulado");
            std::process::exit(3);
        }
        "env" => {
            let seen: Vec<String> = ["TYPESAFE_API_KEY", "JEVALAN_TOKEN", "SOLO_EN_DOTENV"]
                .iter()
                .filter_map(|n| std::env::var(n).ok().map(|v| format!("{n}={v}")))
                .collect();
            let reason = if seen.is_empty() { "none".to_string() } else { seen.join(",") };
            verdict("allow", &reason, "model");
        }
        _ => verdict("allow", "ok", "model"),
    }
}
