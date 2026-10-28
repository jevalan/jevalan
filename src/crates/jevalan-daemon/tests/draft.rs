mod common;

use common::{TempDir, answers, connect, next_json, say, scripted_backend, start_with_flows_and};
use serde_json::{Value, json};

/// El flujo local de `tool.risk`, tal cual se sirve: cuatro riesgos en un `batch` y reglas `route`.
const LOCAL: &str = include_str!("../../jevalan-flow/fixtures/tool-risk-local-v1.yaml");

/// POST con cuerpo JSON; devuelve el estado y la respuesta ya leída.
async fn post(daemon: &str, ruta: &str, cuerpo: Value) -> (u16, Value) {
    let respuesta = reqwest::Client::new().post(format!("{daemon}{ruta}")).json(&cuerpo).send().await.unwrap();
    let estado = respuesta.status().as_u16();
    (estado, respuesta.json().await.unwrap())
}

#[tokio::test]
async fn validate_accepts_a_good_flow_and_names_the_error_of_a_broken_one() {
    let (backend, _guion) = scripted_backend().await;
    let dir = TempDir::with(&[]);
    let daemon = start_with_flows_and(&backend, &["kev"], &dir).await;

    let (estado, bueno) =
        post(&daemon, "/v1/flows/validate", json!({ "flow": "tool-risk-local", "yaml": LOCAL })).await;
    assert_eq!(estado, 200);
    assert_eq!(bueno["ok"], true);
    assert_eq!(bueno["flow"]["name"], "tool-risk-local");
    assert_eq!(bueno["flow"]["nodes"]["risks"]["questions"].as_object().unwrap().len(), 4);

    // Una expresión CEL cortada: el error ha de ser el del parser, y ha de nombrar la regla.
    let roto = LOCAL.replace("answers.risks.destructive >= 0.85", "answers.risks.destructive >");
    let (estado, malo) = post(&daemon, "/v1/flows/validate", json!({ "flow": "tool-risk-local", "yaml": &roto })).await;
    assert_eq!(estado, 200);
    assert_eq!(malo["ok"], false);
    assert!(malo["error"].as_str().unwrap().contains("risks.route"), "{}", malo["error"]);
    assert!(malo.get("flow").is_none());
}

#[tokio::test]
async fn validate_never_touches_the_registry() {
    let (backend, _guion) = scripted_backend().await;
    let dir = TempDir::with(&[]);
    let daemon = start_with_flows_and(&backend, &["kev"], &dir).await;

    let (_, bueno) = post(&daemon, "/v1/flows/validate", json!({ "flow": "tool-risk-local", "yaml": LOCAL })).await;
    assert_eq!(bueno["ok"], true);

    // El flujo es válido, y aun así no se publica: el registro sigue vacío y nada emite FLOW_LOADED.
    let listado: Value = reqwest::get(format!("{daemon}/v1/flows")).await.unwrap().json().await.unwrap();
    assert!(listado["flows"].as_object().unwrap().is_empty(), "{listado}");
    assert!(listado["invalid"].as_object().unwrap().is_empty(), "{listado}");
}

/// Una entrada que pasa las dos reglas fijas y llega al nodo `batch`.
fn entrada() -> Value {
    json!({ "tool_name": "Bash", "tool_input": { "command": "ls -la" } })
}

/// Grabaciones de un `batch`: una por pregunta, con la clave completa `"risks.<pregunta>"`.
fn cassette(p: [f64; 4]) -> Value {
    let claves = ["destructive", "exfiltration", "privilege", "secret_exposure"];
    Value::Object(
        claves
            .into_iter()
            .zip(p)
            .map(|(clave, p)| {
                let completa = format!("risks.{clave}");
                (
                    completa.clone(),
                    json!({
                        "status": 200,
                        "body": { "answers": { (completa.clone()): { "type": "noul", "noul": p, "confidence": 0.9 } } }
                    }),
                )
            })
            .collect(),
    )
}

#[tokio::test]
async fn draft_run_walks_the_borrador_and_leaves_the_registry_untouched() {
    let (backend, guion) = scripted_backend().await;
    say(
        &guion,
        "risks",
        200,
        answers(&[
            ("risks.destructive", "noul", json!(0.9), 0.9),
            ("risks.exfiltration", "noul", json!(0.1), 0.9),
            ("risks.privilege", "noul", json!(0.1), 0.9),
            ("risks.secret_exposure", "noul", json!(0.1), 0.9),
        ]),
    );
    let dir = TempDir::with(&[]);
    let daemon = start_with_flows_and(&backend, &["kev"], &dir).await;

    let (estado, ran) = post(
        &daemon,
        "/v1/flows/draft/run",
        json!({ "flow": "tool-risk-local", "yaml": LOCAL, "input": entrada(), "transport": "live" }),
    )
    .await;

    assert_eq!(estado, 200);
    assert_eq!(ran["flow"], "tool-risk-local");
    // La salida `deny` de este flujo declara `outcome: ok` con `verdict: deny` en sus datos.
    assert_eq!((ran["outcome"].as_str(), ran["data"]["verdict"].as_str()), (Some("ok"), Some("deny")));
    // El camino: las dos reglas fijas y el nodo `batch`, que enruta a `deny`.
    assert_eq!(
        ran["path"].as_array().unwrap().iter().map(|p| p["node"].as_str().unwrap()).collect::<Vec<_>>(),
        ["rm_root", "pipe_sh", "risks"]
    );
    assert_eq!(ran["path"][2]["port"], "deny");
    assert!(ran["confidence"].as_f64().is_some(), "la confianza de la ejecución viaja: {ran}");

    let listado: Value = reqwest::get(format!("{daemon}/v1/flows")).await.unwrap().json().await.unwrap();
    assert!(listado["flows"].as_object().unwrap().is_empty(), "{listado}");
}

#[tokio::test]
async fn draft_run_marks_its_events_as_borrador_and_never_emits_flow_loaded() {
    let (backend, guion) = scripted_backend().await;
    say(
        &guion,
        "risks",
        200,
        answers(&[
            ("risks.destructive", "noul", json!(0.1), 0.9),
            ("risks.exfiltration", "noul", json!(0.1), 0.9),
            ("risks.privilege", "noul", json!(0.1), 0.9),
            ("risks.secret_exposure", "noul", json!(0.1), 0.9),
        ]),
    );
    let dir = TempDir::with(&[]);
    let daemon = start_with_flows_and(&backend, &["kev"], &dir).await;

    let mut ws = connect(&daemon).await;
    assert_eq!(next_json(&mut ws).await["type"], "BACKENDS");

    let (estado, _) = post(
        &daemon,
        "/v1/flows/draft/run",
        json!({ "flow": "tool-risk-local", "yaml": LOCAL, "input": entrada(), "transport": "live" }),
    )
    .await;
    assert_eq!(estado, 200);

    let mut tipos = Vec::new();
    let mut flujos = Vec::new();
    loop {
        let evento = next_json(&mut ws).await;
        tipos.push(evento["type"].as_str().unwrap_or_default().to_string());
        if let Some(f) = evento["payload"]["flow"].as_str() {
            flujos.push(f.to_string());
        }
        if evento["type"] == "DECISION_MADE" {
            break;
        }
    }
    assert!(!tipos.contains(&"FLOW_LOADED".to_string()), "{tipos:?}");
    assert!(flujos.iter().all(|f| f.ends_with(" (borrador)")), "{flujos:?}");
    assert!(flujos.iter().any(|f| f == "tool-risk-local (borrador)"), "{flujos:?}");
}

#[tokio::test]
async fn draft_run_replays_a_cassette_without_touching_a_backend() {
    let (backend, _guion) = scripted_backend().await;
    let dir = TempDir::with(&[]);
    let daemon = start_with_flows_and(&backend, &["kev"], &dir).await;

    let (estado, ran) = post(
        &daemon,
        "/v1/flows/draft/run",
        json!({
            "flow": "tool-risk-local", "yaml": LOCAL, "input": entrada(), "transport": "replay",
            "cassette": cassette([0.1, 0.1, 0.1, 0.1]),
        }),
    )
    .await;

    assert_eq!(estado, 200);
    assert_eq!(ran["data"]["verdict"], "allow");
    assert_eq!(ran["path"][2]["answer"]["destructive"], 0.1);
}

#[tokio::test]
async fn draft_run_refuses_an_unknown_transport_and_an_invalid_draft() {
    let (backend, _guion) = scripted_backend().await;
    let dir = TempDir::with(&[]);
    let daemon = start_with_flows_and(&backend, &["kev"], &dir).await;

    let (estado, malo) = post(
        &daemon,
        "/v1/flows/draft/run",
        json!({ "flow": "tool-risk-local", "yaml": LOCAL, "input": entrada(), "transport": "telepatía" }),
    )
    .await;
    assert_eq!(estado, 400);
    assert!(malo["error"].as_str().unwrap().contains("transporte desconocido"), "{malo}");

    let roto = LOCAL.replace("type: batch", "type: batalla");
    let (estado, malo) = post(
        &daemon,
        "/v1/flows/draft/run",
        json!({ "flow": "tool-risk-local", "yaml": roto, "input": entrada(), "transport": "live" }),
    )
    .await;
    assert_eq!(estado, 422);
    assert!(malo["error"].as_str().unwrap().contains("batalla"), "{malo}");
}
