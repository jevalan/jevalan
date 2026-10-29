//! Fake System One backend: answers consistent with each kind of question, latency and failures as desired.

use crate::config::{Backend, Config, Hub};
use crate::rng::Rng;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

struct Mock {
    rng: Mutex<Rng>,
    latency_ms: (u64, u64),
    fail_rate: f64,
    down: Arc<AtomicBool>,
}

/// A running mock: its base URL and the switch that "takes it down" (it answers 500 immediately).
pub struct MockHandle {
    pub url: String,
    pub down: Arc<AtomicBool>,
}

/// Switch of a demo provider: a mock that is "taken down", or a file whose presence makes the specialist fail.
pub enum Switch {
    Mock(Arc<AtomicBool>),
    Flag(PathBuf),
}

impl Switch {
    /// Flips it and returns whether the provider ends up down.
    pub fn toggle(&self) -> bool {
        match self {
            Switch::Mock(down) => !down.fetch_not(Ordering::Relaxed),
            Switch::Flag(path) if path.exists() => std::fs::remove_file(path).is_err(),
            Switch::Flag(path) => std::fs::write(path, b"").is_ok(),
        }
    }
}

/// Switches of the demo providers, by name.
pub type Switches = BTreeMap<String, Switch>;

/// Starts the mock on `127.0.0.1:0`.
pub async fn spawn_mock(seed: u64, latency_ms: (u64, u64), fail_rate: f64) -> std::io::Result<MockHandle> {
    let down = Arc::new(AtomicBool::new(false));
    let mock = Arc::new(Mock { rng: Mutex::new(Rng::new(seed)), latency_ms, fail_rate, down: down.clone() });
    let app = Router::new().route("/v1/systemone", post(answer)).with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(MockHandle { url, down })
}

/// Demo configuration: `typesafe` (with a dummy key), `ollama` and `openrouter` (the one for `agent-wakeup`)
/// point to separate mocks, each with its own switch, and the hub
/// registers `agent.wakeup` and `tool.risk` (the latter with `specialist` as the `exec` command) in a temporary directory.
pub async fn demo_config(addr: SocketAddr, seed: u64, specialist: Vec<String>) -> std::io::Result<(Config, Switches)> {
    let mut backends = BTreeMap::new();
    let mut switches = Switches::new();
    for (i, name) in ["typesafe", "ollama", "openrouter"].into_iter().enumerate() {
        let MockHandle { url, down } = spawn_mock(seed.wrapping_add(i as u64), (30, 400), 0.05).await?;
        switches.insert(name.to_string(), Switch::Mock(down));
        let api_key = (name == "typesafe").then(|| "demo-key".to_string());
        backends.insert(
            name.to_string(),
            Backend { url, path: "/v1/systemone".into(), api_key, timeout: Duration::from_secs(5) },
        );
    }
    let (capabilities_dir, flag) = write_capabilities(seed, specialist)?;
    switches.insert("toolgate".into(), Switch::Flag(flag));
    let hub = Hub { capabilities_dir, allow_exec: true, ..Hub::default() };
    let cfg = Config {
        addr,
        token: None,
        backends,
        default_backend: "ollama".into(),
        flows_dir: PathBuf::from("./config/flows"),
        hub,
    };
    Ok((cfg, switches))
}

/// Writes the demo's two capabilities and returns their directory and the file that switches the specialist off.
fn write_capabilities(seed: u64, mut specialist: Vec<String>) -> std::io::Result<(PathBuf, PathBuf)> {
    let dir = std::env::temp_dir().join(format!("jevalan-demo-{}-{seed:x}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let flag = dir.join("toolgate.down");
    specialist.extend(["--fail-flag".into(), flag.display().to_string()]);
    let schema = |text: &str| serde_json::from_str::<Value>(text).unwrap_or_default();
    let caps = [
        (
            "agent.wakeup",
            json!({
                "manifest_version": "2",
                // It is served by a flow of the hub itself: it is not publishable, its publisher is local.
                "namespace": "local.jev",
                "version": "1",
                "description": "¿Merece la pena despertar a un agente dormido?",
                "requires": { "runtime": "none", "platform": "any" },
                "input_schema": schema(include_str!("../../../../config/capabilities/schemas/agent.wakeup.input.json")),
                "decision_schema": schema(include_str!("../../../../config/capabilities/schemas/agent.wakeup.decision.json")),
                "provider": { "id": "jevalan-flow", "flow": "agent-wakeup" },
                "defaults": { "deadline_ms": 3000 },
            }),
        ),
        (
            "tool.risk",
            json!({
                "manifest_version": "2",
                "namespace": "local.jev",
                "version": "1",
                "description": "Riesgo de una llamada a herramienta (especialista simulado)",
                // The stand-in specialist is a binary, not Toolgate's CLI with Node.
                "requires": { "runtime": "none", "platform": "any" },
                "input_schema": schema(include_str!("../../../../config/capabilities/schemas/tool.risk.input.json")),
                "decision_schema": schema(include_str!("../../../../config/capabilities/schemas/tool.risk.decision.json")),
                "provider": {
                    "id": "toolgate",
                    "exec": { "command": specialist },
                    "abstain_when": "response.source == 'no-opinion'",
                    "error_when": "response.source == 'fail-mode'",
                },
                "defaults": { "deadline_ms": 5000 },
            }),
        ),
    ];
    for (name, cap) in caps {
        let yaml = serde_yaml::to_string(&cap).map_err(std::io::Error::other)?;
        std::fs::write(dir.join(format!("{name}.yaml")), yaml)?;
    }
    Ok((dir, flag))
}

async fn answer(State(mock): State<Arc<Mock>>, body: Bytes) -> Response {
    if mock.down.load(Ordering::Relaxed) {
        return reply_json(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "mock caído" }));
    }
    // Everything random is decided before the first `await`: the lock does not cross awaits.
    let (delay, fail, reply) = {
        let mut rng = mock.rng.lock().unwrap_or_else(PoisonError::into_inner);
        let delay = rng.range(mock.latency_ms.0, mock.latency_ms.1);
        let fail = rng.unit() < mock.fail_rate;
        (delay, fail, build_reply(&body, &mut rng))
    };
    tokio::time::sleep(Duration::from_millis(delay)).await;
    if fail {
        return reply_json(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "mock failure" }));
    }
    match reply {
        Ok(v) => reply_json(StatusCode::OK, v),
        Err(e) => reply_json(StatusCode::BAD_REQUEST, json!({ "error": e })),
    }
}

fn reply_json(status: StatusCode, v: Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response()
}

fn build_reply(body: &[u8], rng: &mut Rng) -> Result<Value, String> {
    let req: Value = serde_json::from_slice(body).map_err(|e| format!("JSON inválido: {e}"))?;
    let questions = req["questions"].as_object().filter(|q| !q.is_empty()).ok_or("faltan «questions»")?;
    let mut answers = Map::new();
    for (name, q) in questions {
        answers.insert(name.clone(), answer_to(name, q, rng)?);
    }
    let n = questions.len() as u64;
    let usage = json!({ "input_tokens": req["state"].to_string().len() as u64 / 4 + 12 * n, "output_tokens": 4 * n });
    Ok(json!({ "model": req["model"], "answers": answers, "usage": usage }))
}

fn answer_to(name: &str, q: &Value, rng: &mut Rng) -> Result<Value, String> {
    match q["type"].as_str() {
        Some("noul") => {
            // Almost always harmless, now and then emphatic: this way the flows walk through all their branches.
            let p = if rng.unit() < 0.85 { rng.between(0.0, 0.4) } else { rng.between(0.85, 1.0) };
            Ok(json!({ "type": "noul", "noul": round(p), "confidence": confidence(rng) }))
        }
        Some("choice") => {
            let options = q["criteria"].as_object().filter(|c| !c.is_empty());
            let keys: Vec<&String> = options.ok_or_else(|| format!("«{name}»: falta criteria"))?.keys().collect();
            let weights: Vec<f64> = keys.iter().map(|_| rng.between(0.05, 1.0)).collect();
            let total: f64 = weights.iter().sum();
            let probs: Vec<f64> = weights.iter().map(|w| w / total).collect();
            let mut draw = rng.unit();
            let mut chosen = keys.len() - 1;
            for (i, p) in probs.iter().enumerate() {
                if draw < *p {
                    chosen = i;
                    break;
                }
                draw -= p;
            }
            let probabilities: Map<String, Value> =
                keys.iter().zip(&probs).map(|(k, p)| ((*k).clone(), json!(p))).collect();
            let confidence = confidence(rng);
            Ok(
                json!({ "type": "choice", "choice": keys[chosen], "probabilities": probabilities, "confidence": confidence }),
            )
        }
        Some("score") => {
            let levels = q["criteria"].as_array().map_or(0, Vec::len);
            if levels == 0 {
                return Err(format!("«{name}»: falta criteria"));
            }
            let score = rng.between(0.0, (levels - 1) as f64);
            Ok(json!({ "type": "score", "score": round(score), "confidence": confidence(rng) }))
        }
        other => Err(format!("«{name}»: tipo desconocido {other:?}")),
    }
}

fn confidence(rng: &mut Rng) -> f64 {
    round(rng.between(0.4, 0.99))
}

fn round(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn post(url: &str) -> reqwest::Response {
        let body = json!({ "model": "m", "questions": { "q": { "type": "noul" } } });
        reqwest::Client::new().post(format!("{url}/v1/systemone")).json(&body).send().await.unwrap()
    }

    #[tokio::test]
    async fn down_mock_returns_500_and_recovers() {
        let mock = spawn_mock(1, (0, 1), 0.0).await.unwrap();
        mock.down.store(true, Ordering::Relaxed);
        let resp = post(&mock.url).await;
        assert_eq!(resp.status(), 500);
        assert_eq!(resp.json::<Value>().await.unwrap()["error"], "mock caído");
        mock.down.store(false, Ordering::Relaxed);
        let resp = post(&mock.url).await;
        assert_eq!(resp.status(), 200);
        assert!(resp.json::<Value>().await.unwrap()["answers"]["q"]["noul"].is_number());
    }
}
