//! Capa de protocolo: JSON-RPC 2.0 de MCP, sin red ni E/S, para poder probarla a pelo.

use serde_json::{Value, json};

/// Versiones de MCP que se saben hablar; la primera es la que se anuncia si el cliente pide otra.
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Una herramienta tal como la ve `tools/list`.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Resultado de una llamada: el valor tipado y si debe marcarse como fallo.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub value: Value,
    pub is_error: bool,
}

/// Error de protocolo (no de herramienta): código JSON-RPC y motivo.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolError {
    pub code: i64,
    pub message: String,
}

/// Lo que el servidor necesita del exterior; los tests lo sustituyen por un simulacro.
pub trait Daemon {
    fn list_tools(&self) -> Vec<Tool>;
    fn call(&self, name: &str, args: Value) -> Result<ToolOutcome, ProtocolError>;
}

fn reply(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn failure(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// Aviso de que la lista de herramientas cambió.
pub fn list_changed_notification() -> String {
    json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }).to_string()
}

/// Atiende una línea de entrada. `None` para líneas vacías y notificaciones (mensajes sin `id`).
pub fn handle_line(daemon: &dyn Daemon, line: &str) -> Option<String> {
    if line.trim().is_empty() {
        return None;
    }
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return Some(failure(&Value::Null, -32700, "JSON inválido"));
    };
    let id = msg.get("id")?.clone();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let answer = match msg.get("method").and_then(Value::as_str) {
        Some("initialize") => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
            let version = SUPPORTED_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&SUPPORTED_VERSIONS[0]);
            reply(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": true } },
                    "serverInfo": { "name": "jevalan", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
        }
        Some("ping") => reply(&id, json!({})),
        Some("tools/list") => {
            let tools: Vec<Value> = daemon
                .list_tools()
                .into_iter()
                .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema }))
                .collect();
            reply(&id, json!({ "tools": tools }))
        }
        Some("tools/call") => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match daemon.call(name, args) {
                Ok(out) => reply(
                    &id,
                    json!({
                        "content": [{ "type": "text", "text": out.value.to_string() }],
                        "structuredContent": out.value,
                        "isError": out.is_error,
                    }),
                ),
                Err(e) => failure(&id, e.code, &e.message),
            }
        }
        _ => failure(&id, -32601, "método desconocido"),
    };
    Some(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    struct FakeDaemon;

    impl Daemon for FakeDaemon {
        fn list_tools(&self) -> Vec<Tool> {
            vec![Tool { name: "echo".into(), description: "Devuelve lo recibido".into(), input_schema: json!({"type":"object"}) }]
        }
        fn call(&self, name: &str, args: Value) -> Result<ToolOutcome, ProtocolError> {
            match name {
                "echo" => Ok(ToolOutcome { value: json!({ "got": args }), is_error: false }),
                _ => Err(ProtocolError { code: -32602, message: format!("herramienta desconocida: {name}") }),
            }
        }
    }

    fn ask(line: &str) -> Value {
        serde_json::from_str(&handle_line(&FakeDaemon, line).expect("debía responder")).unwrap()
    }

    #[test]
    fn initialize_negotiates_version() {
        let r = ask(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#);
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["capabilities"]["tools"]["listChanged"], true);
        assert_eq!(r["result"]["serverInfo"]["name"], "jevalan");
        let r = ask(r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#);
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn tools_list_returns_daemon_tools() {
        let r = ask(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        assert_eq!(r["result"]["tools"][0]["name"], "echo");
        assert_eq!(r["result"]["tools"][0]["inputSchema"], json!({"type":"object"}));
    }

    #[test]
    fn tools_call_wraps_value() {
        let r = ask(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"a":1}}}"#);
        assert_eq!(r["result"]["structuredContent"], json!({"got":{"a":1}}));
        assert_eq!(r["result"]["isError"], false);
        let text: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, json!({"got":{"a":1}}));
    }

    #[test]
    fn unknown_tool_is_invalid_params() {
        let r = ask(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope"}}"#);
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn unknown_method_is_not_found() {
        assert_eq!(ask(r#"{"jsonrpc":"2.0","id":5,"method":"nope"}"#)["error"]["code"], -32601);
    }

    #[test]
    fn malformed_line_gets_parse_error_with_null_id() {
        let r = ask("esto no es json");
        assert_eq!(r["error"]["code"], -32700);
        assert!(r["id"].is_null());
    }

    #[test]
    fn notifications_get_no_reply() {
        assert!(handle_line(&FakeDaemon, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(handle_line(&FakeDaemon, r#"{"jsonrpc":"2.0","method":"cualquiera"}"#).is_none());
        assert!(handle_line(&FakeDaemon, "   ").is_none());
    }

    #[test]
    fn ping_returns_empty_object() {
        assert_eq!(ask(r#"{"jsonrpc":"2.0","id":6,"method":"ping"}"#)["result"], json!({}));
    }
}
