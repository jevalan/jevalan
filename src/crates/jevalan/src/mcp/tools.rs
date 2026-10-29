//! Tool catalogue: the hub's capabilities, plus four tools for flows.

use super::daemon::Http;
use super::protocol::{Daemon, ProtocolError, Tool, ToolOutcome};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

const RUN_FLOW: &str = "jevalan_run_flow";
const LIST_FLOWS: &str = "jevalan_list_flows";
const VALIDATE_FLOW: &str = "jevalan_validate_flow";
const TRY_DRAFT: &str = "jevalan_try_draft";
const STATIC: [&str; 4] = [RUN_FLOW, LIST_FLOWS, VALIDATE_FLOW, TRY_DRAFT];

/// MCP name of a capability: every character outside `[A-Za-z0-9_-]` becomes `_` (`tool.risk` → `tool_risk`).
pub fn tool_name(capability: &str) -> String {
    capability.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// A flow name that fits in a path without escaping from it.
fn valid_flow_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

struct CapTool {
    name: String,
    capability: String,
    version: String,
    description: String,
    schema: Value,
}

pub struct Catalog<H: Http> {
    http: H,
    allow_live: bool,
    counter: Cell<u64>,
    warned: RefCell<HashSet<String>>,
}

fn failed(message: impl Into<String>) -> ToolOutcome {
    ToolOutcome { value: json!({ "error": message.into() }), is_error: true }
}

fn answered(reply: Result<Value, String>) -> ToolOutcome {
    match reply {
        Ok(value) => ToolOutcome { value, is_error: false },
        Err(why) => failed(why),
    }
}

impl<H: Http> Catalog<H> {
    pub fn new(http: H, allow_live: bool) -> Self {
        Self { http, allow_live, counter: Cell::new(0), warned: RefCell::default() }
    }

    /// `true` the first time a colliding tool name is seen, so the warning is printed once, not on every poll.
    fn note_collision(&self, name: &str) -> bool {
        self.warned.borrow_mut().insert(name.to_string())
    }

    /// One tool per loaded capability; if two share a name the first is kept.
    fn capability_tools(&self) -> Result<Vec<CapTool>, String> {
        let listing = self.http.get("/v1/hub/capabilities")?;
        let Some(caps) = listing.get("capabilities").and_then(Value::as_object) else { return Ok(Vec::new()) };
        let mut out: Vec<CapTool> = Vec::new();
        for (id, summary) in caps {
            let name = tool_name(id);
            if STATIC.contains(&name.as_str()) || out.iter().any(|t| t.name == name) {
                if self.note_collision(&name) {
                    eprintln!("jevalan mcp: la capacidad «{id}» se omite: su nombre «{name}» ya está en uso");
                }
                continue;
            }
            let text = |k: &str| summary.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            out.push(CapTool {
                name,
                capability: id.clone(),
                version: text("version"),
                description: text("description"),
                schema: summary.get("input_schema").cloned().unwrap_or_else(|| json!({ "type": "object" })),
            });
        }
        Ok(out)
    }

    fn call_capability(&self, cap: &CapTool, args: Value) -> ToolOutcome {
        let n = self.counter.get() + 1;
        self.counter.set(n);
        let body = json!({
            "protocol_version": "jev-hub/0.1",
            "request_id": format!("mcp-{}-{n}", std::process::id()),
            "capability": cap.capability,
            "capability_version": cap.version,
            "input": args,
        });
        match self.http.post("/v1/hub", &body) {
            Ok(value) => {
                let is_error = value.get("status").and_then(Value::as_str) == Some("error");
                ToolOutcome { value, is_error }
            }
            Err(why) => failed(why),
        }
    }

    fn run_flow(&self, args: &Value) -> ToolOutcome {
        let Some(flow) = args.get("flow").and_then(Value::as_str).filter(|f| valid_flow_name(f)) else {
            return failed("«flow» debe ser un nombre de flujo válido (letras, cifras, «-», «_» y «.»)");
        };
        let state = args.get("state").cloned().unwrap_or_else(|| json!({}));
        answered(self.http.post(&format!("/v1/decide/{flow}"), &json!({ "state": state, "client": "mcp" })))
    }

    fn flow_and_yaml(args: &Value) -> Result<(&str, &str), ToolOutcome> {
        let flow = args.get("flow").and_then(Value::as_str).filter(|f| valid_flow_name(f));
        let yaml = args.get("yaml").and_then(Value::as_str);
        match (flow, yaml) {
            (Some(f), Some(y)) => Ok((f, y)),
            _ => Err(failed("hacen falta «flow» (nombre válido) y «yaml» (texto)")),
        }
    }

    fn validate_flow(&self, args: &Value) -> ToolOutcome {
        match Self::flow_and_yaml(args) {
            Ok((flow, yaml)) => answered(self.http.post("/v1/flows/validate", &json!({ "flow": flow, "yaml": yaml }))),
            Err(e) => e,
        }
    }

    fn try_draft(&self, args: &Value) -> ToolOutcome {
        let (flow, yaml) = match Self::flow_and_yaml(args) {
            Ok(pair) => pair,
            Err(e) => return e,
        };
        let transport = match args.get("transport") {
            None => "replay",
            Some(Value::String(s)) => s.as_str(),
            Some(_) => return failed("«transport» debe ser un texto: replay, live o record"),
        };
        if !["replay", "live", "record"].contains(&transport) {
            return failed(format!("transporte desconocido: {transport}"));
        }
        if transport != "replay" && !self.allow_live {
            return failed(format!(
                "el transporte «{transport}» llama a modelos reales y está apagado: arranca «jevalan mcp --allow-live» para usarlo"
            ));
        }
        let mut body = json!({
            "flow": flow, "yaml": yaml,
            "input": args.get("input").cloned().unwrap_or_else(|| json!({})),
            "transport": transport,
        });
        if let Some(cassette) = args.get("cassette") {
            body["cassette"] = cassette.clone();
        }
        answered(self.http.post("/v1/flows/draft/run", &body))
    }

    fn static_tools(&self) -> Vec<Tool> {
        let transports = if self.allow_live { json!(["replay", "live", "record"]) } else { json!(["replay"]) };
        let tool = |name: &str, description: &str, schema: Value| Tool {
            name: name.into(),
            description: description.into(),
            input_schema: schema,
        };
        vec![
            tool(
                RUN_FLOW,
                "Ejecuta un flujo publicado por su nombre con un «state» y devuelve su resultado y el camino recorrido.",
                json!({ "type": "object", "required": ["flow"], "properties": {
                    "flow": { "type": "string", "description": "Nombre del flujo (fichero sin extensión)" },
                    "state": { "description": "Entrada del flujo; las expresiones la leen como «input»" } } }),
            ),
            tool(
                LIST_FLOWS,
                "Lista los flujos cargados y los inválidos con su motivo.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool(
                VALIDATE_FLOW,
                "Valida el YAML de un flujo con el analizador real, sin publicarlo. Un flujo inválido devuelve ok=false y el motivo.",
                json!({ "type": "object", "required": ["flow", "yaml"], "properties": {
                    "flow": { "type": "string" }, "yaml": { "type": "string" } } }),
            ),
            tool(
                TRY_DRAFT,
                "Ejecuta una vez un borrador de flujo sin publicarlo.",
                json!({ "type": "object", "required": ["flow", "yaml", "input"], "properties": {
                    "flow": { "type": "string" }, "yaml": { "type": "string" }, "input": {},
                    "transport": { "type": "string", "enum": transports },
                    "cassette": { "type": "object", "description": "Grabaciones para el transporte replay" } } }),
            ),
        ]
    }
}

impl<H: Http> Daemon for Catalog<H> {
    fn list_tools(&self) -> Vec<Tool> {
        let mut tools = self.static_tools();
        tools.extend(self.capability_tools().unwrap_or_default().into_iter().map(|c| Tool {
            name: c.name,
            description: c.description,
            input_schema: c.schema,
        }));
        tools
    }

    fn call(&self, name: &str, args: Value) -> Result<ToolOutcome, ProtocolError> {
        match name {
            RUN_FLOW => Ok(self.run_flow(&args)),
            LIST_FLOWS => Ok(answered(self.http.get("/v1/flows"))),
            VALIDATE_FLOW => Ok(self.validate_flow(&args)),
            TRY_DRAFT => Ok(self.try_draft(&args)),
            // With the daemon down it is not known whether the tool exists: the reason is given, not "unknown".
            _ => match self.capability_tools() {
                Err(why) => Ok(failed(why)),
                Ok(caps) => match caps.into_iter().find(|c| c.name == name) {
                    Some(cap) => Ok(self.call_capability(&cap, args)),
                    None => Err(ProtocolError { code: -32602, message: format!("herramienta desconocida: {name}") }),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use std::collections::HashMap;

    type Reply = Result<Value, String>;

    #[derive(Default)]
    struct FakeHttp {
        gets: HashMap<String, Reply>,
        posts: HashMap<String, Reply>,
        log: RefCell<Vec<(String, String, Option<Value>)>>,
    }

    impl Http for FakeHttp {
        fn get(&self, path: &str) -> Reply {
            self.log.borrow_mut().push(("GET".into(), path.into(), None));
            self.gets.get(path).cloned().unwrap_or_else(|| Err("daemon inalcanzable en prueba".into()))
        }
        fn post(&self, path: &str, body: &Value) -> Reply {
            self.log.borrow_mut().push(("POST".into(), path.into(), Some(body.clone())));
            self.posts.get(path).cloned().unwrap_or_else(|| Err("daemon inalcanzable en prueba".into()))
        }
    }

    fn caps(names: &[&str]) -> Value {
        let mut m = serde_json::Map::new();
        for n in names {
            m.insert(
                (*n).into(),
                json!({ "version": "1", "description": format!("cap {n}"), "input_schema": { "type": "object", "required": ["x"] } }),
            );
        }
        json!({ "capabilities": m, "invalid": {} })
    }

    fn http_with(names: &[&str]) -> FakeHttp {
        let mut h = FakeHttp::default();
        h.gets.insert("/v1/hub/capabilities".into(), Ok(caps(names)));
        h
    }

    fn names(c: &Catalog<FakeHttp>) -> Vec<String> {
        c.list_tools().into_iter().map(|t| t.name).collect()
    }

    #[test]
    fn tool_name_replaces_dots() {
        assert_eq!(tool_name("tool.risk"), "tool_risk");
        assert_eq!(tool_name("a b/c"), "a_b_c");
    }

    #[test]
    fn lists_static_and_capability_tools() {
        let c = Catalog::new(http_with(&["tool.risk", "agent.wakeup"]), false);
        let mut got = names(&c);
        got.sort();
        assert_eq!(
            got,
            [
                "agent_wakeup",
                "jevalan_list_flows",
                "jevalan_run_flow",
                "jevalan_try_draft",
                "jevalan_validate_flow",
                "tool_risk"
            ]
        );
        let risk = c.list_tools().into_iter().find(|t| t.name == "tool_risk").unwrap();
        assert_eq!(risk.input_schema, json!({ "type": "object", "required": ["x"] }));
        assert_eq!(risk.description, "cap tool.risk");
    }

    #[test]
    fn without_daemon_only_static_tools_are_listed() {
        let c = Catalog::new(FakeHttp::default(), false);
        assert_eq!(names(&c).len(), 4);
    }

    #[test]
    fn colliding_names_keep_the_first_and_do_not_panic() {
        let c = Catalog::new(http_with(&["a.b", "a_b"]), false);
        assert_eq!(names(&c).iter().filter(|n| *n == "a_b").count(), 1);
        let c = Catalog::new(http_with(&["jevalan.run_flow"]), false);
        assert_eq!(names(&c).iter().filter(|n| *n == "jevalan_run_flow").count(), 1);
    }

    #[test]
    fn capability_call_posts_envelope() {
        let mut h = http_with(&["tool.risk"]);
        h.posts.insert("/v1/hub".into(), Ok(json!({ "status": "ok", "decision": { "verdict": "allow" } })));
        let c = Catalog::new(h, false);
        let out = c.call("tool_risk", json!({ "x": 1 })).unwrap();
        assert!(!out.is_error);
        assert_eq!(out.value["decision"]["verdict"], "allow");
        let log = c.http.log.borrow();
        let (_, _, body) = log.iter().find(|(_, p, _)| p == "/v1/hub").unwrap();
        let body = body.as_ref().unwrap();
        assert_eq!(body["capability"], "tool.risk");
        assert_eq!(body["capability_version"], "1");
        assert_eq!(body["protocol_version"], "jev-hub/0.1");
        assert_eq!(body["input"], json!({ "x": 1 }));
        assert!(body["request_id"].as_str().unwrap().starts_with("mcp-"));
    }

    #[test]
    fn abstain_is_not_an_error_but_hub_error_is() {
        let mut h = http_with(&["tool.risk"]);
        h.posts.insert("/v1/hub".into(), Ok(json!({ "status": "abstain", "abstain": { "reason": "n/a" } })));
        assert!(!Catalog::new(h, false).call("tool_risk", json!({})).unwrap().is_error);
        let mut h = http_with(&["tool.risk"]);
        h.posts.insert("/v1/hub".into(), Ok(json!({ "status": "error", "error": { "message": "x" } })));
        assert!(Catalog::new(h, false).call("tool_risk", json!({})).unwrap().is_error);
    }

    #[test]
    fn unreachable_daemon_is_error_outcome_not_protocol_error() {
        let out = Catalog::new(FakeHttp::default(), false).call("jevalan_list_flows", json!({})).unwrap();
        assert!(out.is_error);
        assert!(out.value["error"].as_str().unwrap().contains("inalcanzable"));
    }

    #[test]
    fn draft_live_without_flag_is_refused_before_any_request() {
        for transport in ["live", "record"] {
            let c = Catalog::new(FakeHttp::default(), false);
            let out = c
                .call("jevalan_try_draft", json!({ "flow": "x", "yaml": "y", "input": {}, "transport": transport }))
                .unwrap();
            assert!(out.is_error);
            assert!(out.value["error"].as_str().unwrap().contains("--allow-live"));
            assert!(c.http.log.borrow().is_empty());
        }
        let mut h = FakeHttp::default();
        h.posts.insert("/v1/flows/draft/run".into(), Ok(json!({ "outcome": "ok" })));
        let c = Catalog::new(h, true);
        let out =
            c.call("jevalan_try_draft", json!({ "flow": "x", "yaml": "y", "input": {}, "transport": "live" })).unwrap();
        assert!(!out.is_error);
        assert_eq!(c.http.log.borrow().len(), 1);
    }

    #[test]
    fn draft_transport_enum_depends_on_the_flag() {
        let schema = |allow| {
            let c = Catalog::new(FakeHttp::default(), allow);
            c.list_tools().into_iter().find(|t| t.name == "jevalan_try_draft").unwrap().input_schema["properties"]["transport"]
                ["enum"]
                .clone()
        };
        assert_eq!(schema(false), json!(["replay"]));
        assert_eq!(schema(true), json!(["replay", "live", "record"]));
    }

    #[test]
    fn run_flow_posts_state_to_decide() {
        let mut h = FakeHttp::default();
        h.posts.insert("/v1/decide/deploy-gate".into(), Ok(json!({ "outcome": "deny" })));
        let c = Catalog::new(h, false);
        let out = c.call("jevalan_run_flow", json!({ "flow": "deploy-gate", "state": { "env": "prod" } })).unwrap();
        assert_eq!(out.value["outcome"], "deny");
        let log = c.http.log.borrow();
        assert_eq!(log[0].2.as_ref().unwrap(), &json!({ "state": { "env": "prod" }, "client": "mcp" }));
    }

    #[test]
    fn flow_names_that_could_escape_the_path_are_refused() {
        let c = Catalog::new(FakeHttp::default(), false);
        for bad in ["../x", "a/b", "", "..", "a b"] {
            let out = c.call("jevalan_run_flow", json!({ "flow": bad, "state": {} })).unwrap();
            assert!(out.is_error, "{bad}");
        }
        assert!(c.http.log.borrow().is_empty());
    }

    #[test]
    fn validate_flow_verdict_is_not_an_error() {
        let mut h = FakeHttp::default();
        h.posts.insert("/v1/flows/validate".into(), Ok(json!({ "ok": false, "error": "mal" })));
        let out = Catalog::new(h, false).call("jevalan_validate_flow", json!({ "flow": "x", "yaml": "y" })).unwrap();
        assert!(!out.is_error);
        assert_eq!(out.value["ok"], false);
    }

    #[test]
    fn unknown_tool_is_invalid_params() {
        let err = Catalog::new(http_with(&[]), false).call("nope", json!({})).unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[test]
    fn capability_call_with_the_daemon_down_is_an_error_outcome_not_unknown_tool() {
        let out = Catalog::new(FakeHttp::default(), false).call("tool_risk", json!({})).unwrap();
        assert!(out.is_error);
        assert!(out.value["error"].as_str().unwrap().contains("inalcanzable"));
    }

    #[test]
    fn a_collision_is_reported_only_the_first_time() {
        let c = Catalog::new(FakeHttp::default(), false);
        assert!(c.note_collision("a_b"));
        assert!(!c.note_collision("a_b"));
        assert!(c.note_collision("c_d"));
    }

    #[test]
    fn a_non_string_transport_is_rejected_before_any_request() {
        for t in [json!(5), json!(null), json!(["live"])] {
            let c = Catalog::new(FakeHttp::default(), true);
            let out =
                c.call("jevalan_try_draft", json!({ "flow": "x", "yaml": "y", "input": {}, "transport": t })).unwrap();
            assert!(out.is_error, "{t}");
            assert!(c.http.log.borrow().is_empty());
        }
    }
}
