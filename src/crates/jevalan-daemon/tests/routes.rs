//! Routes: a node names an ordered list of `(backend, model)` and the first member that answers decides.

mod common;

use common::{Scripted, TempDir, answer, cfg_with, say, scripted_backend};
use jevalan_daemon::config::{Member, Route};
use jevalan_daemon::{AppState, router};
use serde_json::{Value, json};

const FLOW: &str = r#"
description: ruta
defaults: { backend: local }
start: q
nodes:
  q: { type: noul, instructions: "¿Es destructivo?", out: { "true": no, "false": yes, error: bad } }
outputs:
  yes: { outcome: allow }
  no: { outcome: deny }
  bad: { outcome: abstain }
"#;

/// A port where nothing listens: connecting to it is refused at once.
const DEAD: &str = "http://127.0.0.1:1";

fn member(backend: &str, model: &str) -> Member {
    Member { backend: backend.into(), model: model.into() }
}

/// Daemon with the flow above over the route `local`, whose members are `members` (backend name, url, model).
async fn daemon(members: &[(&str, &str, &str)]) -> (String, TempDir) {
    let backends: Vec<(&str, &str, Option<&str>)> = members.iter().map(|(n, url, _)| (*n, *url, None)).collect();
    let mut cfg = cfg_with(&backends, None);
    let dir = TempDir::with(&[("route.yaml", FLOW)]);
    cfg.flows_dir = dir.0.clone();
    cfg.routes.insert(
        "local".into(),
        Route { allow_cloud: false, members: members.iter().map(|(n, _, m)| member(n, m)).collect() },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(cfg);
    let watcher = state.load_flows().unwrap();
    tokio::spawn(async move {
        let _watcher = watcher;
        axum::serve(listener, router(state)).await
    });
    (format!("http://{addr}"), dir)
}

async fn decide(daemon: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{daemon}/v1/decide/route"))
        .json(&json!({ "state": "ls -la" }))
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

fn received(script: &Scripted) -> Vec<Value> {
    script.log.lock().unwrap().iter().map(|c| serde_json::from_slice(&c.body).unwrap()).collect()
}

#[tokio::test]
async fn a_dead_first_member_hands_over_to_the_second() {
    let (live, script) = scripted_backend().await;
    say(&script, "q", 200, answer("q", "noul", json!(0.1), 0.9));
    let (daemon, _dir) = daemon(&[("kev", DEAD, "kev-latest"), ("laya", &live, "laya-rl")]).await;

    let (status, reply) = decide(&daemon).await;

    assert_eq!((status, reply["outcome"].as_str()), (200, Some("allow")));
    let step = &reply["path"][0];
    assert_eq!((step["backend"].as_str(), step["model"].as_str()), (Some("laya"), Some("laya-rl")));
    assert_eq!(step["tried"], json!(["kev"]));
    // The request that reached the second member carries the member's model, not the first one's.
    assert_eq!(received(&script)[0]["model"], "laya-rl");
}

#[tokio::test]
async fn a_live_first_member_is_the_only_one_asked() {
    let (first, one) = scripted_backend().await;
    let (second, two) = scripted_backend().await;
    say(&one, "q", 200, answer("q", "noul", json!(0.1), 0.9));
    say(&two, "q", 200, answer("q", "noul", json!(0.1), 0.9));
    let (daemon, _dir) = daemon(&[("kev", &first, "k"), ("laya", &second, "l")]).await;

    let (_, reply) = decide(&daemon).await;

    assert_eq!(reply["path"][0]["backend"], "kev");
    assert!(reply["path"][0].get("tried").is_none(), "nothing fell back, so nothing is said");
    assert!(received(&two).is_empty());
}

#[tokio::test]
async fn an_http_error_does_not_hand_over() {
    let (first, one) = scripted_backend().await;
    let (second, two) = scripted_backend().await;
    say(&one, "q", 503, "{}".into());
    say(&two, "q", 200, answer("q", "noul", json!(0.1), 0.9));
    let (daemon, _dir) = daemon(&[("kev", &first, "k"), ("laya", &second, "l")]).await;

    let (_, reply) = decide(&daemon).await;

    assert_eq!((reply["outcome"].as_str(), reply["path"][0]["port"].as_str()), (Some("abstain"), Some("error")));
    assert!(received(&two).is_empty(), "a 5xx is the node's to judge, not a reason to try another model");
}

#[tokio::test]
async fn a_route_with_every_member_down_takes_the_error_port() {
    let (daemon, _dir) = daemon(&[("kev", DEAD, "k"), ("laya", DEAD, "l")]).await;

    let (status, reply) = decide(&daemon).await;

    assert_eq!((status, reply["outcome"].as_str()), (200, Some("abstain")));
    let step = &reply["path"][0];
    assert_eq!(step["port"], "error");
    assert_eq!(step["tried"], json!(["kev", "laya"]));
    assert_eq!(step["backend"], "local", "nobody answered, so the node keeps the name it declared");
}
