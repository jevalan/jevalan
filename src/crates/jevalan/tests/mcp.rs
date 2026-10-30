//! End-to-end MCP session: the `jevalan mcp` binary against a real daemon of its own.
//!
//! It does not touch the user's `daemon.json` or `shadow.jsonl`: the daemon is started in-process, without
//! writing the discovery file, and the child receives the address through `--addr`.

use jevalan_daemon::config::{Backend, Config, Hub};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const DEPLOY_GATE: &str = r#"
description: "Gate a deployment with plain rules"
defaults: { backend: ollama, model: nimble }
start: prod_from_branch
nodes:
  prod_from_branch:
    type: rule
    when: 'input.env == "prod" && input.branch != "main"'
    out: { "true": deny_branch, "false": allow }
outputs:
  deny_branch: { outcome: deny, data: { reason: "production deploys must come from main" } }
  allow:       { outcome: allow }
"#;

const DEPLOY_CAPABILITY: &str = r#"
manifest_version: "2"
namespace: local.test
requires: { runtime: none, platform: any }
version: "1"
description: "Barrera de despliegue"
input_schema: { type: object }
decision_schema: { type: object }
provider: { id: jevalan-flow, flow: deploy-gate }
defaults: { deadline_ms: 3000 }
"#;

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("jevalan-mcp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn free_addr() -> std::net::SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

fn config(addr: std::net::SocketAddr, flows: &Path, caps: &Path, signals: &Path) -> Config {
    let backend = Backend {
        url: "http://127.0.0.1:9".into(),
        path: "/v1/systemone".into(),
        api_key: None,
        timeout: Duration::from_millis(500),
        cloud: false,
    };
    Config {
        addr,
        token: Some("t0ken".into()),
        backends: BTreeMap::from([("ollama".to_string(), backend)]),
        routes: BTreeMap::new(),
        sessions: Default::default(),
        default_backend: "ollama".into(),
        flows_dir: flows.to_path_buf(),
        hub: Hub { capabilities_dir: caps.to_path_buf(), signals_dir: signals.to_path_buf(), ..Hub::default() },
    }
}

fn session(addr: std::net::SocketAddr, home: &Path, requests: &[Value]) -> (Vec<Value>, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jevalan"))
        .args(["mcp", "--addr", &addr.to_string(), "--token", "t0ken"])
        .env("LOCALAPPDATA", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("JEVALAN_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for r in requests {
            writeln!(stdin, "{r}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let replies = stdout.lines().map(|l| serde_json::from_str(l).expect("stdout es solo protocolo")).collect();
    (replies, stdout, stderr)
}

fn call(id: u64, name: &str, args: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } })
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_session_against_a_real_daemon() {
    let scratch = Scratch::new("e2e");
    let (flows, caps, signals, home) = (
        scratch.path().join("flows"),
        scratch.path().join("caps"),
        scratch.path().join("signals"),
        scratch.path().join("home"),
    );
    for d in [&flows, &caps, &signals, &home] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(flows.join("deploy-gate.yaml"), DEPLOY_GATE).unwrap();
    std::fs::write(caps.join("deploy.gate.yaml"), DEPLOY_CAPABILITY).unwrap();

    let addr = free_addr();
    let server =
        tokio::spawn(jevalan_daemon::serve_quiet(config(addr, &flows, &caps, &signals), std::future::pending()));
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let requests = vec![
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        call(
            3,
            "jevalan_run_flow",
            json!({ "flow": "deploy-gate", "state": { "env": "prod", "branch": "feature/x" } }),
        ),
        call(4, "jevalan_validate_flow", json!({ "flow": "x", "yaml": "esto: [no es un flujo" })),
        call(5, "jevalan_try_draft", json!({ "flow": "x", "yaml": DEPLOY_GATE, "input": {}, "transport": "live" })),
        call(6, "deploy_gate", json!({ "env": "prod", "branch": "feature/x" })),
    ];
    let home_for_child = home.clone();
    let (replies, stdout, stderr) =
        tokio::task::spawn_blocking(move || session(addr, &home_for_child, &requests)).await.unwrap();
    server.abort();

    assert_eq!(replies.len(), 6, "la notificación no se contesta: {stdout}");
    let by_id =
        |id: u64| replies.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("falta la respuesta {id}: {stdout}"));

    assert_eq!(by_id(1)["result"]["protocolVersion"], "2025-06-18");
    let names: Vec<&str> =
        by_id(2)["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"jevalan_run_flow") && names.contains(&"deploy_gate"), "{names:?}");

    assert_eq!(by_id(3)["result"]["structuredContent"]["outcome"], "deny");
    assert_eq!(by_id(3)["result"]["isError"], false);

    assert_eq!(by_id(4)["result"]["structuredContent"]["ok"], false);
    assert_eq!(by_id(4)["result"]["isError"], false, "un flujo inválido es un veredicto, no un fallo");

    assert_eq!(by_id(5)["result"]["isError"], true);
    assert!(by_id(5)["result"]["content"][0]["text"].as_str().unwrap().contains("--allow-live"));

    assert_eq!(by_id(6)["result"]["isError"], false, "{}", by_id(6));
    assert_eq!(by_id(6)["result"]["structuredContent"]["status"], "ok", "{}", by_id(6));

    assert!(!stdout.contains("t0ken") && !stderr.contains("t0ken"), "el token no debe salir: {stdout}{stderr}");
}
