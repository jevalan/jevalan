mod common;

use common::{cfg_with, fake_backend, fake_backend_at, start_daemon};
use futures_util::StreamExt;
use serde_json::Value;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const OK_BODY: &str = r#"{"answers":{"q":{"value":1}},"usage":{"input_tokens":7}}"#;
const REQUEST: &str = r#"{"state":"secreto-del-usuario","model":"nimble","questions":{"q":{}}}"#;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn body_is_forwarded_byte_for_byte_and_response_returned_untouched() {
    let (url, log) = fake_backend(200, OK_BODY).await;
    let daemon = start_daemon(cfg_with(&[("a", &url, None)], None)).await;
    let raw = "{ \"state\":\"x\",  \"model\":\"nimble\", \"questions\":{} }";

    let resp = client().post(format!("{daemon}/a/v1/systemone")).body(raw).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), OK_BODY);
    let log = log.lock().unwrap();
    assert_eq!(&log[0].body[..], raw.as_bytes());
}

#[tokio::test]
async fn backend_path_decides_where_the_request_lands() {
    let (url, log) = fake_backend_at("/api/alpha/decisions", 200, OK_BODY).await;
    let mut cfg = cfg_with(&[("a", &url, Some("k"))], None);
    cfg.backends.get_mut("a").unwrap().path = "/api/alpha/decisions".into();
    let daemon = start_daemon(cfg).await;

    let resp = client().post(format!("{daemon}/a/v1/systemone")).body(REQUEST).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    let log = log.lock().unwrap();
    assert_eq!((log.len(), &log[0].body[..]), (1, REQUEST.as_bytes()));
    assert_eq!(log[0].headers["authorization"], "Bearer k");
}

#[tokio::test]
async fn api_key_is_injected() {
    let (url, log) = fake_backend(200, OK_BODY).await;
    let daemon = start_daemon(cfg_with(&[("a", &url, Some("k"))], None)).await;

    client().post(format!("{daemon}/a/v1/systemone")).body(REQUEST).send().await.unwrap();

    assert_eq!(log.lock().unwrap()[0].headers["authorization"], "Bearer k");
}

#[tokio::test]
async fn client_authorization_is_never_forwarded() {
    let (plain_url, plain_log) = fake_backend(200, OK_BODY).await;
    let (keyed_url, keyed_log) = fake_backend(200, OK_BODY).await;
    let daemon = start_daemon(cfg_with(&[("plain", &plain_url, None), ("keyed", &keyed_url, Some("k"))], None)).await;

    for path in ["plain", "keyed"] {
        client()
            .post(format!("{daemon}/{path}/v1/systemone"))
            .header("Authorization", "Bearer cliente")
            .body(REQUEST)
            .send()
            .await
            .unwrap();
    }

    assert!(plain_log.lock().unwrap()[0].headers.get("authorization").is_none());
    assert_eq!(keyed_log.lock().unwrap()[0].headers["authorization"], "Bearer k");
}

#[tokio::test]
async fn default_route_uses_default_backend() {
    let (url, log) = fake_backend(200, OK_BODY).await;
    let daemon = start_daemon(cfg_with(&[("main", &url, None)], None)).await;

    let resp = client().post(format!("{daemon}/v1/systemone")).body(REQUEST).send().await.unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_backend_is_404() {
    let (url, log) = fake_backend(200, OK_BODY).await;
    let daemon = start_daemon(cfg_with(&[("a", &url, None)], None)).await;

    let resp = client().post(format!("{daemon}/nope/v1/systemone")).body(REQUEST).send().await.unwrap();

    assert_eq!(resp.status(), 404);
    let json: Value = resp.json().await.unwrap();
    assert_eq!(json["error"], "backend desconocido: nope");
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn token_is_required_when_configured() {
    let (url, _log) = fake_backend(200, OK_BODY).await;
    let daemon = start_daemon(cfg_with(&[("a", &url, None)], Some("t"))).await;
    let post = |auth: Option<&'static str>| {
        let mut req = client().post(format!("{daemon}/a/v1/systemone")).body(REQUEST);
        if let Some(a) = auth {
            req = req.header("Authorization", a);
        }
        req.send()
    };

    assert_eq!(post(None).await.unwrap().status(), 401);
    assert_eq!(post(Some("Bearer mal")).await.unwrap().status(), 401);
    assert_eq!(post(Some("Bearer t")).await.unwrap().status(), 200);
    assert_eq!(client().get(format!("{daemon}/healthz")).send().await.unwrap().status(), 200);
}

#[tokio::test]
async fn body_over_64_kib_is_413() {
    let (url, log) = fake_backend(200, OK_BODY).await;
    let daemon = start_daemon(cfg_with(&[("a", &url, None)], None)).await;

    let resp = client().post(format!("{daemon}/a/v1/systemone")).body(vec![b'x'; 64 * 1024 + 1]).send().await.unwrap();

    assert_eq!(resp.status(), 413);
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn backend_status_is_propagated() {
    let (url, _log) = fake_backend(500, r#"{"error":"x"}"#).await;
    let daemon = start_daemon(cfg_with(&[("a", &url, None)], None)).await;

    let resp = client().post(format!("{daemon}/a/v1/systemone")).body(REQUEST).send().await.unwrap();

    assert_eq!(resp.status(), 500);
    assert_eq!(resp.text().await.unwrap(), r#"{"error":"x"}"#);
}

#[tokio::test]
async fn unreachable_backend_is_502() {
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let daemon = start_daemon(cfg_with(&[("a", &url, None)], None)).await;

    let resp = client().post(format!("{daemon}/a/v1/systemone")).body(REQUEST).send().await.unwrap();

    assert_eq!(resp.status(), 502);
    let json: Value = resp.json().await.unwrap();
    assert!(json["error"].is_string());
}

fn ws_url(daemon: &str, query: &str) -> String {
    format!("ws://{}/events{query}", daemon.trim_start_matches("http://"))
}

#[tokio::test]
async fn passthrough_emits_its_events_in_order() {
    let (url, _log) = fake_backend(200, OK_BODY).await;
    let daemon = start_daemon(cfg_with(&[("a", &url, Some("clave-del-backend"))], None)).await;
    let (mut ws, _) = connect_async(ws_url(&daemon, "")).await.unwrap();

    client()
        .post(format!("{daemon}/a/v1/systemone"))
        .header("X-Jevalan-Client", "crush")
        .body(REQUEST)
        .send()
        .await
        .unwrap();

    let mut events = Vec::new();
    for _ in 0..5 {
        let Message::Text(text) = ws.next().await.unwrap().unwrap() else { panic!("se esperaba texto") };
        assert!(!text.contains("secreto-del-usuario") && !text.contains("clave-del-backend"), "{text}");
        events.push(serde_json::from_str::<Value>(&text).unwrap());
    }
    assert_eq!(events.remove(0)["type"], "BACKENDS");

    let types: Vec<_> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["REQUEST_RECEIVED", "BACKEND_REQUEST", "BACKEND_RESPONSE", "PASSTHROUGH_DONE"]);
    assert!(events.iter().all(|e| e["trace_id"] == events[0]["trace_id"]));
    let seqs: Vec<_> = events.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, [1, 2, 3, 4]);
    assert_eq!(events[0]["payload"]["client"], "crush");
    assert_eq!(events[1]["payload"]["questions"], 1);
    assert_eq!(events[2]["payload"]["input_tokens"], 7);
}

#[tokio::test]
async fn events_require_the_token_too() {
    let (url, _log) = fake_backend(200, OK_BODY).await;
    let daemon = start_daemon(cfg_with(&[("a", &url, None)], Some("t"))).await;

    assert!(connect_async(ws_url(&daemon, "")).await.is_err());
    assert!(connect_async(ws_url(&daemon, "?token=mal")).await.is_err());
    assert!(connect_async(ws_url(&daemon, "?token=t")).await.is_ok());
}
