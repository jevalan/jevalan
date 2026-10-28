use jevalan_daemon::mock::spawn_mock;
use jevalan_flow::{parse, reply_from_response};
use serde_json::{Value, json};

const NOUL: &str = r#"
defaults: { model: m }
start: q
nodes:
  q: { type: noul, instructions: "?", out: { "true": a, "false": a } }
outputs:
  a: { outcome: allow }
"#;
const CHOICE: &str = r#"
defaults: { model: m }
start: q
nodes:
  q: { type: choice, instructions: "?", criteria: { x: X, y: Y, z: Z }, out: { x: a, y: a, z: a } }
outputs:
  a: { outcome: allow }
"#;
const SCORE: &str = r#"
defaults: { model: m }
start: q
nodes:
  q: { type: score, instructions: "?", criteria: [a, b, c, d, e], out: { "0..4": a } }
outputs:
  a: { outcome: allow }
"#;

fn node_flow(yaml: &str) -> jevalan_flow::Flow {
    parse("t", yaml, &["ollama"], "ollama").unwrap_or_else(|e| {
        panic!(
            "{e:?}
{yaml}"
        )
    })
}

async fn ask(url: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{url}/v1/systemone"))
        .header("Authorization", "Bearer dummy")
        .json(&body)
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap_or_default())
}

#[tokio::test]
async fn answers_are_accepted_for_every_question_type() {
    let url = spawn_mock(7, (0, 1), 0.0).await.unwrap().url;
    for (ty, yaml) in [("noul", NOUL), ("choice", CHOICE), ("score", SCORE)] {
        let flow = node_flow(yaml);
        let node = &flow.nodes["q"];
        let question = jevalan_flow::question(node);
        for _ in 0..20 {
            let body = jevalan_flow::request_body("m", &json!("estado"), "q", &question);
            let (status, resp) = ask(&url, body).await;
            assert_eq!(status, 200, "{ty}: {resp}");
            assert_eq!(resp["model"], "m");
            assert!(resp["usage"]["input_tokens"].is_u64() && resp["usage"]["output_tokens"].is_u64());
            let reply = reply_from_response("q", node, &resp).unwrap_or_else(|e| panic!("{ty}: {e}\n{resp}"));
            assert!((0.0..=1.0).contains(&reply.confidence));
            match ty {
                "score" => assert!((0.0..=4.0).contains(&reply.answer.as_f64().unwrap())),
                "noul" => assert!((0.0..=1.0).contains(&reply.answer.as_f64().unwrap())),
                _ => {
                    let sum: f64 = resp["answers"]["q"]["probabilities"]
                        .as_object()
                        .unwrap()
                        .values()
                        .map(|v| v.as_f64().unwrap())
                        .sum();
                    assert!((sum - 1.0).abs() < 1e-9, "{sum}");
                    assert!(["x", "y", "z"].contains(&reply.answer.as_str().unwrap()));
                }
            }
        }
    }
}

fn valid() -> Value {
    json!({ "model": "m", "state": "s", "questions": { "q": { "type": "noul", "instructions": "?" } } })
}

#[tokio::test]
async fn full_fail_rate_always_gives_500() {
    let url = spawn_mock(1, (0, 1), 1.0).await.unwrap().url;
    let (status, body) = ask(&url, valid()).await;
    assert_eq!((status, body["error"].as_str()), (500, Some("mock failure")));
}

#[tokio::test]
async fn zero_fail_rate_never_gives_500() {
    let url = spawn_mock(2, (0, 1), 0.0).await.unwrap().url;
    for _ in 0..20 {
        assert_eq!(ask(&url, valid()).await.0, 200);
    }
}

#[tokio::test]
async fn malformed_requests_give_400() {
    let url = spawn_mock(3, (0, 1), 0.0).await.unwrap().url;
    let raw = reqwest::Client::new().post(format!("{url}/v1/systemone")).body("no es json").send().await.unwrap();
    assert_eq!(raw.status(), 400);
    let (status, body) = ask(&url, json!({ "model": "m" })).await;
    assert_eq!(status, 400);
    assert!(body["error"].is_string());
    let (status, _) = ask(&url, json!({ "questions": { "q": { "type": "choice" } } })).await;
    assert_eq!(status, 400);
}
