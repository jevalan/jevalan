//! Stand-in `tool.risk` specialist, shaped like Toolgate: silly rules over `tool_input`.

use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

/// FNV-1a: deterministic across runs, which is what matters here.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

pub fn decide(input: &Value, fail: bool) -> Value {
    let verdict =
        |verdict: &str, reason: &str, source: &str| json!({ "verdict": verdict, "reason": reason, "source": source });
    if fail {
        return verdict("ask", "especialista caído", "fail-mode");
    }
    let tool_input = input["tool_input"].to_string();
    let seed = input["tool_use_id"].as_str().map_or_else(|| tool_input.clone(), String::from);
    if hash(&seed) % 100 < 5 {
        return verdict("passthrough", "sin opinión", "no-opinion");
    }
    let pipes_to_shell = tool_input.contains("curl") && (tool_input.contains("| sh") || tool_input.contains("| bash"));
    if tool_input.contains("rm -rf") || pipes_to_shell {
        verdict("deny", "comando destructivo", "rules")
    } else if input["tool_name"] == "Bash" {
        verdict("ask", "ejecuta una orden de shell", "rules")
    } else {
        verdict("allow", "sin riesgo aparente", "rules")
    }
}

pub fn run(capability: &str, fail: bool, fail_flag: Option<&Path>) -> ExitCode {
    if capability != "tool.risk" {
        eprintln!("capacidad desconocida: {capability}");
        return ExitCode::from(2);
    }
    let mut text = String::new();
    let _ = std::io::stdin().read_to_string(&mut text);
    let input: Value = serde_json::from_str(&text).unwrap_or_default();
    let down = fail || fail_flag.is_some_and(Path::exists);
    println!("{}", decide(&input, down));
    ExitCode::SUCCESS
}
