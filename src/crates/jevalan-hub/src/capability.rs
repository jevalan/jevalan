use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Component, Path};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityDef {
    manifest_version: String,
    namespace: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    publisher: Option<PublisherDef>,
    #[serde(default)]
    requires: RequiresDef,
    input_schema: Value,
    decision_schema: Value,
    provider: ProviderDef,
    defaults: DefaultsDef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublisherDef {
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    #[default]
    Any,
    Unix,
    Windows,
}

/// Declared needs, never bindings: the effective command and variables
/// live in `provider`, which is not published. Declaring `platform: unix` on Windows
/// invalidates nothing; refusing to use it is the consumer's decision.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiresDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<String>,
    #[serde(default)]
    pub platform: Platform,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DefaultsDef {
    deadline_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderDef {
    id: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    flow: Option<String>,
    #[serde(default)]
    exec: Option<ExecDef>,
    #[serde(default)]
    http: Option<HttpDef>,
    #[serde(default)]
    abstain_when: Option<String>,
    #[serde(default)]
    error_when: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecDef {
    command: Vec<String>,
    #[serde(default)]
    env: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpDef {
    url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Provider {
    Flow { flow: String },
    Exec { command: Vec<String>, env: Vec<String> },
    Http { url: String },
}

impl Provider {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Flow { .. } => "flow",
            Self::Exec { .. } => "exec",
            Self::Http { .. } => "http",
        }
    }
}

/// Validated capability. The JSON Schema validators are shared between clones.
#[derive(Clone)]
pub struct Capability {
    pub name: String,
    pub namespace: String,
    pub version: String,
    pub description: String,
    pub publisher: Option<PublisherDef>,
    pub requires: RequiresDef,
    pub provider_id: String,
    pub provider_version: Option<String>,
    pub provider: Provider,
    pub abstain_when: Option<String>,
    pub error_when: Option<String>,
    pub deadline_ms: u64,
    pub input_schema: Value,
    pub decision_schema: Value,
    /// See `Capability::digest`.
    digest: String,
    input: Arc<jsonschema::Validator>,
    decision: Arc<jsonschema::Validator>,
}

impl fmt::Debug for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Capability({} v{} → {})", self.name, self.version, self.provider_id)
    }
}

impl Capability {
    pub fn check_input(&self, v: &Value) -> Result<(), String> {
        first_error(&self.input, v)
    }

    pub fn check_decision(&self, v: &Value) -> Result<(), String> {
        first_error(&self.decision, v)
    }

    /// Global identifier: the publisher's namespace plus the file name.
    pub fn id(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    /// `sha256:<hex>` of the contract: the resolved capability (schemas inlined, not their `$file` reference) without
    /// the `provider` block, so that rebinding a capability never changes it and pinning by hash pins a contract.
    /// `serde_json` keeps object keys sorted, which makes the serialization canonical on any machine.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// What `/v1/hub/capabilities` publishes: never the command, environment or URL.
    pub fn summary(&self) -> Value {
        json!({
            "id": self.id(),
            "digest": self.digest,
            "namespace": self.namespace,
            "publisher": self.publisher,
            "requires": self.requires,
            "version": self.version,
            "description": self.description,
            "provider": { "id": self.provider_id, "type": self.provider.kind() },
            "input_schema": self.input_schema,
            "decision_schema": self.decision_schema,
            "defaults": { "deadline_ms": self.deadline_ms },
        })
    }
}

fn first_error(v: &jsonschema::Validator, value: &Value) -> Result<(), String> {
    match v.iter_errors(value).next() {
        Some(e) => Err(e.to_string()),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CapError {
    Yaml(String),
    BadManifest(String),
    BadName(String),
    BadSchema(String),
    BadFile(String),
    BadProvider(String),
    BadExpression(String),
}

impl fmt::Display for CapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Yaml(e) => write!(f, "YAML inválido: {e}"),
            Self::BadManifest(e) => write!(f, "manifiesto inválido: {e}"),
            Self::BadName(n) => write!(f, "nombre de capacidad inválido: {n:?}"),
            Self::BadSchema(e) => write!(f, "esquema inválido: {e}"),
            Self::BadFile(e) => write!(f, "$file inválido: {e}"),
            Self::BadProvider(e) => write!(f, "proveedor inválido: {e}"),
            Self::BadExpression(e) => write!(f, "expresión CEL inválida: {e}"),
        }
    }
}

impl std::error::Error for CapError {}

pub fn valid_capability_name(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
}

/// Without variables: a `${NAME}` in `exec.command` stays as it is.
pub fn parse_capability(
    name: &str,
    yaml: &str,
    base_dir: &Path,
    flow_exists: &dyn Fn(&str) -> bool,
) -> Result<Capability, CapError> {
    parse_capability_with(name, yaml, base_dir, flow_exists, &|_| None, false)
}

/// Like `parse_capability`, but resolves `${NAME}` in `exec.command` with `vars` at load time (never with data from
/// a request). With `strict`, an undefined or empty variable invalidates the capability; without it, the
/// literal is left and the capability is still valid (though useless, since its host does not run `exec`).
pub fn parse_capability_with(
    name: &str,
    yaml: &str,
    base_dir: &Path,
    flow_exists: &dyn Fn(&str) -> bool,
    vars: &dyn Fn(&str) -> Option<String>,
    strict: bool,
) -> Result<Capability, CapError> {
    if !valid_capability_name(name) {
        return Err(CapError::BadName(name.into()));
    }
    let def: CapabilityDef = serde_yaml::from_str(yaml).map_err(|e| CapError::Yaml(e.to_string()))?;
    if def.manifest_version != "2" {
        return Err(CapError::BadManifest(format!(
            "manifest_version {:?} no soportada: se espera \"2\". El manifiesto v1 no declara namespace ni requires",
            def.manifest_version
        )));
    }
    if def.namespace.trim().is_empty() {
        return Err(CapError::BadManifest("namespace es obligatorio y no puede estar vacío".into()));
    }
    if def.version.is_empty() || def.provider.id.is_empty() {
        return Err(CapError::BadProvider("version y provider.id son obligatorios".into()));
    }
    let input_schema = resolve_file(def.input_schema, base_dir)?;
    let decision_schema = resolve_file(def.decision_schema, base_dir)?;
    let input = jsonschema::validator_for(&input_schema).map_err(|e| CapError::BadSchema(e.to_string()))?;
    let decision = jsonschema::validator_for(&decision_schema).map_err(|e| CapError::BadSchema(e.to_string()))?;
    for expr in [&def.provider.abstain_when, &def.provider.error_when].into_iter().flatten() {
        crate::program::compiled(expr).map_err(CapError::BadExpression)?;
    }
    let mut p = def.provider;
    if let Some(e) = p.exec.as_mut() {
        e.command = e.command.iter().map(|arg| expand(arg, vars, strict)).collect::<Result<_, _>>()?;
    }
    let provider = match (p.flow, p.exec, p.http) {
        (Some(flow), None, None) if flow_exists(&flow) => Provider::Flow { flow },
        (Some(flow), None, None) => return Err(CapError::BadProvider(format!("flujo desconocido: {flow}"))),
        (None, Some(e), None) if !e.command.is_empty() && is_batch(&e.command[0]) => {
            let msg = "un .cmd/.bat pasaría por cmd.exe; usa [\"node\", \"<ruta>/dist/cli.js\", \"decide\"]";
            return Err(CapError::BadProvider(msg.into()));
        }
        (None, Some(e), None) if !e.command.is_empty() => Provider::Exec { command: e.command, env: e.env },
        (None, Some(_), None) => return Err(CapError::BadProvider("exec.command está vacío".into())),
        (None, None, Some(h)) if h.url.starts_with("http://") || h.url.starts_with("https://") => {
            Provider::Http { url: h.url }
        }
        (None, None, Some(_)) => return Err(CapError::BadProvider("http.url debe ser http(s)".into())),
        _ => return Err(CapError::BadProvider("declara exactamente uno de flow, exec o http".into())),
    };
    let contract = json!({
        "name": name,
        "namespace": def.namespace,
        "version": def.version,
        "description": def.description,
        "publisher": def.publisher,
        "requires": def.requires,
        "input_schema": input_schema,
        "decision_schema": decision_schema,
        "deadline_ms": def.defaults.deadline_ms,
    });
    let hash = Sha256::digest(contract.to_string().as_bytes());
    let digest = format!("sha256:{}", hash.iter().map(|b| format!("{b:02x}")).collect::<String>());
    Ok(Capability {
        name: name.into(),
        namespace: def.namespace,
        version: def.version,
        description: def.description,
        publisher: def.publisher,
        requires: def.requires,
        provider_id: p.id,
        provider_version: p.version,
        provider,
        abstain_when: p.abstain_when,
        error_when: p.error_when,
        deadline_ms: def.defaults.deadline_ms,
        input_schema,
        decision_schema,
        digest,
        input: Arc::new(input),
        decision: Arc::new(decision),
    })
}

/// Substitutes each `${NAME}` (`[A-Z_][A-Z0-9_]*`) in `arg`; see `parse_capability_with`.
fn expand(arg: &str, vars: &dyn Fn(&str) -> Option<String>, strict: bool) -> Result<String, CapError> {
    let bad = |m: String| CapError::BadProvider(m);
    let (mut out, mut rest) = (String::new(), arg);
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 2..];
        let end = tail.find('}').ok_or_else(|| bad(format!("exec.command: «{arg}» deja un ${{ sin cerrar")))?;
        let name = &tail[..end];
        let mut chars = name.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_uppercase() || c == '_')
            && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !valid {
            return Err(bad(format!("exec.command: «${{{name}}}» no es un nombre de variable válido")));
        }
        match vars(name).filter(|v| !v.is_empty()) {
            Some(v) => out.push_str(&v),
            None if strict => return Err(bad(format!("exec.command usa ${{{name}}}, que no está definida"))),
            None => out.push_str(&format!("${{{name}}}")),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A `.cmd` / `.bat` would route the launch through `cmd.exe`, that is, through a command interpreter.
pub fn is_batch(program: &str) -> bool {
    let lower = program.to_ascii_lowercase();
    lower.ends_with(".cmd") || lower.ends_with(".bat")
}

/// `{ $file: "path" }` → the JSON of that file, relative to `base_dir` and never leaving it.
fn resolve_file(v: Value, base_dir: &Path) -> Result<Value, CapError> {
    let Some(rel) = v.as_object().filter(|o| o.len() == 1).and_then(|o| o.get("$file")).and_then(Value::as_str) else {
        return Ok(v);
    };
    let path = Path::new(rel);
    if path.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(CapError::BadFile(format!("{rel}: debe ser relativa y sin '..'")));
    }
    let text = std::fs::read_to_string(base_dir.join(path)).map_err(|e| CapError::BadFile(format!("{rel}: {e}")))?;
    serde_json::from_str(&text).map_err(|e| CapError::BadFile(format!("{rel}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/capabilities")
    }

    fn load(name: &str, yaml: &str) -> Result<Capability, CapError> {
        parse_capability(name, yaml, &dir(), &|f| f == "agent-wakeup")
    }

    #[test]
    fn example_capabilities_load() {
        for name in ["agent.wakeup", "tool.risk"] {
            let yaml = std::fs::read_to_string(dir().join(format!("{name}.yaml"))).unwrap();
            load(name, &yaml).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn names_need_a_dot() {
        assert!(valid_capability_name("tool.risk"));
        assert!(valid_capability_name("a-b.c-d.e"));
        assert!(!valid_capability_name("tool"));
        assert!(!valid_capability_name("Tool.Risk"));
        assert!(!valid_capability_name("tool..risk"));
    }

    const EXEC: &str = r#"
manifest_version: "2"
namespace: io.github.prueba
version: "1"
input_schema: { type: object, required: [tool_name] }
decision_schema: { type: object, required: [verdict] }
provider:
  id: toolgate
  exec: { command: ["toolgate", "decide"], env: [TYPESAFE_API_KEY] }
  abstain_when: "response.source == 'no-opinion'"
defaults: { deadline_ms: 5000 }
"#;

    #[test]
    fn exec_provider_parses_and_validates_payloads() {
        let c = load("tool.risk", EXEC).unwrap();
        assert!(
            matches!(&c.provider, Provider::Exec { command, env } if command[0] == "toolgate" && env == &["TYPESAFE_API_KEY"])
        );
        assert!(c.check_input(&json!({"tool_name": "Bash"})).is_ok());
        assert!(c.check_input(&json!({})).is_err());
        assert!(c.check_decision(&json!({})).is_err());
    }

    #[test]
    fn summary_hides_command_and_env() {
        let s = load("tool.risk", EXEC).unwrap().summary().to_string();
        assert!(!s.contains("toolgate\", \"decide") && !s.contains("TYPESAFE_API_KEY"), "{s}");
        assert!(s.contains("\"exec\""));
    }

    #[test]
    fn digest_covers_the_contract_and_not_the_binding() {
        let base = load("tool.risk", EXEC).unwrap();
        assert!(base.digest().starts_with("sha256:") && base.digest().len() == 71, "{}", base.digest());
        assert_eq!(base.digest(), load("tool.risk", EXEC).unwrap().digest());
        // Rebinding the provider, its command or its environment leaves the contract alone.
        let rebound = EXEC.replace("toolgate\", \"decide", "otro\", \"decide").replace("TYPESAFE_API_KEY", "OTRA");
        assert_eq!(base.digest(), load("tool.risk", &rebound).unwrap().digest());
        // A different contract, or the same one under another name, does not share it.
        let other = EXEC.replace("required: [verdict]", "required: [verdict, reason]");
        assert_ne!(base.digest(), load("tool.risk", &other).unwrap().digest());
        assert_ne!(base.digest(), load("tool.other", EXEC).unwrap().digest());
        assert_eq!(base.id(), "io.github.prueba/tool.risk");
        assert!(base.summary().to_string().contains(base.digest()));
    }

    #[test]
    fn broken_cel_is_invalid_not_a_panic() {
        let y = EXEC.replace("response.source == 'no-opinion'", "response. &&");
        assert!(matches!(load("tool.risk", &y), Err(CapError::BadExpression(_))));
    }

    #[test]
    fn file_ref_cannot_escape() {
        let y = EXEC.replace("{ type: object, required: [tool_name] }", "{ $file: \"../Cargo.toml\" }");
        assert!(matches!(load("tool.risk", &y), Err(CapError::BadFile(_))));
    }

    #[test]
    fn exactly_one_provider_kind() {
        let both = EXEC.replace("abstain_when", "flow: agent-wakeup\n  abstain_when");
        assert!(matches!(load("tool.risk", &both), Err(CapError::BadProvider(_))));
    }

    #[test]
    fn unknown_flow_is_invalid() {
        let y = EXEC.replace(r#"exec: { command: ["toolgate", "decide"], env: [TYPESAFE_API_KEY] }"#, "flow: nope");
        assert!(matches!(load("tool.risk", &y), Err(CapError::BadProvider(_))));
    }

    #[test]
    fn bad_schema_is_invalid() {
        let y = EXEC.replace("{ type: object, required: [verdict] }", "{ type: 42 }");
        assert!(matches!(load("tool.risk", &y), Err(CapError::BadSchema(_))));
    }

    #[test]
    fn http_url_must_be_http() {
        let y = EXEC.replace(
            r#"exec: { command: ["toolgate", "decide"], env: [TYPESAFE_API_KEY] }"#,
            r#"http: { url: "ftp://x" }"#,
        );
        assert!(matches!(load("tool.risk", &y), Err(CapError::BadProvider(_))));
    }

    #[test]
    fn cmd_and_bat_are_rejected() {
        for prog in ["toolgate.cmd", "C:/x/Tool.BAT"] {
            let y = EXEC.replace("\"toolgate\"", &format!("{prog:?}"));
            assert!(matches!(load("tool.risk", &y), Err(CapError::BadProvider(m)) if m.contains("cmd.exe")), "{prog}");
        }
    }

    fn with_vars(arg: &str, vars: &[(&str, &str)], strict: bool) -> Result<Capability, CapError> {
        let y = EXEC.replace("\"toolgate\"", &format!("{arg:?}"));
        let lookup = |n: &str| vars.iter().find(|(k, _)| *k == n).map(|(_, v)| v.to_string());
        parse_capability_with("tool.risk", &y, &dir(), &|_| false, &lookup, strict)
    }

    fn program(c: &Capability) -> &str {
        match &c.provider {
            Provider::Exec { command, .. } => &command[0],
            _ => panic!("no es exec"),
        }
    }

    #[test]
    fn vars_expand_whole_and_partial() {
        let c = with_vars("${CLI}", &[("CLI", "/opt/x/cli.js")], true).unwrap();
        assert_eq!(program(&c), "/opt/x/cli.js");
        let c = with_vars("--policy=${A}/${B_2}.yaml", &[("A", "dir"), ("B_2", "p")], true).unwrap();
        assert_eq!(program(&c), "--policy=dir/p.yaml");
    }

    #[test]
    fn undefined_or_empty_var_is_invalid_when_strict() {
        for vars in [&[][..], &[("CLI", "")][..]] {
            let e = with_vars("${CLI}", vars, true).unwrap_err();
            assert!(e.to_string().contains("exec.command usa ${CLI}, que no está definida"), "{e}");
        }
    }

    #[test]
    fn undefined_var_is_tolerated_when_lenient() {
        assert_eq!(program(&with_vars("${CLI}", &[], false).unwrap()), "${CLI}");
    }

    #[test]
    fn bad_var_names_are_invalid() {
        for arg in ["${cli}", "${1X}", "${}", "${CLI"] {
            assert!(matches!(with_vars(arg, &[("CLI", "x")], true), Err(CapError::BadProvider(_))), "{arg}");
        }
    }

    #[test]
    fn batch_check_applies_to_expanded_program() {
        let e = with_vars("${CLI}", &[("CLI", "C:/npm/toolgate.cmd")], true).unwrap_err();
        assert!(e.to_string().contains("cmd.exe"), "{e}");
    }
}
