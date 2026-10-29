//! Daemon configuration: YAML file, `JEVALAN_*` variables and defaults.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub addr: SocketAddr,
    pub token: Option<String>,
    pub backends: BTreeMap<String, Backend>,
    /// Ordered `(backend, model)` lists a flow may name where it would name a backend.
    pub routes: BTreeMap<String, Route>,
    pub default_backend: String,
    pub flows_dir: PathBuf,
    pub hub: Hub,
}

/// A route: the first member that is reachable answers. Members that go out to the cloud are already left out
/// unless the route opted in, so nothing downstream has to remember to check.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub allow_cloud: bool,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub backend: String,
    pub model: String,
}

/// Decision hub settings; `exec` stays off unless the host explicitly opts in.
#[derive(Clone)]
pub struct Hub {
    pub capabilities_dir: PathBuf,
    pub allow_exec: bool,
    /// Catalogue of named signals (`config/signals/*.yaml`) that flows reuse.
    pub signals_dir: PathBuf,
    /// Variables from the `.env` read at start-up; they feed the `env` of `exec` providers without touching the process.
    pub dotenv: BTreeMap<String, String>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            capabilities_dir: PathBuf::from("./config/capabilities"),
            allow_exec: false,
            signals_dir: PathBuf::from("./config/signals"),
            dotenv: BTreeMap::new(),
        }
    }
}

// The `.env` values are secrets: only the names are shown.
impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("capabilities_dir", &self.capabilities_dir)
            .field("allow_exec", &self.allow_exec)
            .field("signals_dir", &self.signals_dir)
            .field("dotenv", &self.dotenv.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Clone)]
pub struct Backend {
    pub url: String,
    /// Ruta bajo `url` (por defecto `/v1/systemone`).
    pub path: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
    /// The backend lives outside this machine. Only a route that says `allow_cloud` may reach it.
    pub cloud: bool,
}

// The key must never leak through a `{:?}`.
impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backend")
            .field("url", &self.url)
            .field("path", &self.path)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("timeout", &self.timeout)
            .field("cloud", &self.cloud)
            .finish()
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    server: RawServer,
    backends: Option<BTreeMap<String, RawBackend>>,
    #[serde(default)]
    routes: BTreeMap<String, RawRoute>,
    default_backend: Option<String>,
    flows_dir: Option<PathBuf>,
    #[serde(default)]
    hub: RawHub,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawHub {
    capabilities_dir: Option<PathBuf>,
    allow_exec: Option<bool>,
    signals_dir: Option<PathBuf>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawServer {
    addr: Option<String>,
    token_env: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBackend {
    url: String,
    path: Option<String>,
    api_key_env: Option<String>,
    timeout_ms: Option<u64>,
    #[serde(default)]
    cloud: bool,
}

/// A route is a bare list of members (cloud closed) or, to open the cloud, an object that says so.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawRoute {
    Short(Vec<Member>),
    Long {
        #[serde(default)]
        allow_cloud: bool,
        members: Vec<Member>,
    },
}

/// Address the daemon listens on if nobody says otherwise. It is also the last one its clients
/// ask when the `daemon.json` announcement leads nowhere.
pub const DEFAULT_ADDR: &str = "127.0.0.1:8080";
const DEFAULT_PATH: &str = "/v1/systemone";
const DEFAULT_TIMEOUT_MS: u64 = 5000;

/// Loads the configuration; `env` is injected so it can be tested without touching the real environment.
pub fn load(path: Option<&Path>, env: &dyn Fn(&str) -> Option<String>) -> Result<Config, String> {
    let path = path.map(Path::to_path_buf).or_else(|| env("JEVALAN_CONFIG").map(PathBuf::from));
    let raw = match path {
        Some(p) => {
            let text = std::fs::read_to_string(&p).map_err(|e| format!("no se puede leer {}: {e}", p.display()))?;
            serde_yaml::from_str(&text).map_err(|e| format!("YAML inválido en {}: {e}", p.display()))?
        }
        None => RawConfig::default(),
    };
    build(raw, env)
}

fn valid_name(name: &str) -> bool {
    name != "v1" && !name.is_empty() && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn build(raw: RawConfig, env: &dyn Fn(&str) -> Option<String>) -> Result<Config, String> {
    let addr_text = env("JEVALAN_ADDR").or(raw.server.addr).unwrap_or_else(|| DEFAULT_ADDR.into());
    let addr = addr_text.parse().map_err(|e| format!("dirección inválida «{addr_text}»: {e}"))?;
    let token_env = raw.server.token_env.unwrap_or_else(|| "JEVALAN_TOKEN".into());
    let token = env(&token_env).filter(|t| !t.is_empty());

    let raw_backends = raw.backends.unwrap_or_else(|| {
        let ollama = RawBackend {
            url: "http://localhost:11434".into(),
            path: None,
            api_key_env: None,
            timeout_ms: None,
            cloud: false,
        };
        BTreeMap::from([("ollama".to_string(), ollama)])
    });
    let mut backends = BTreeMap::new();
    for (name, b) in raw_backends {
        if !valid_name(&name) {
            return Err(format!("nombre de backend inválido «{name}»: use [a-z0-9-]+ y distinto de «v1»"));
        }
        if !(b.url.starts_with("http://") || b.url.starts_with("https://")) {
            return Err(format!("la url del backend «{name}» no es http(s): {}", b.url));
        }
        let api_key = match &b.api_key_env {
            Some(var) => match env(var).filter(|k| !k.is_empty()) {
                Some(key) => Some(key),
                None => return Err(format!("el backend «{name}» declara api_key_env «{var}», vacía o ausente")),
            },
            None => None,
        };
        let path = b.path.unwrap_or_else(|| DEFAULT_PATH.into());
        if !path.starts_with('/') || path.contains("..") {
            return Err(format!("el path del backend «{name}» debe empezar por / y no contener ..: {path}"));
        }
        let timeout = Duration::from_millis(b.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
        backends.insert(
            name,
            Backend { url: b.url.trim_end_matches('/').to_string(), path, api_key, timeout, cloud: b.cloud },
        );
    }

    let routes = build_routes(raw.routes, &backends)?;
    let default_backend = env("JEVALAN_DEFAULT_BACKEND").or(raw.default_backend).unwrap_or_else(|| "ollama".into());
    if !backends.contains_key(&default_backend) && !routes.contains_key(&default_backend) {
        return Err(format!("default_backend «{default_backend}» no figura entre los backends ni las rutas"));
    }
    let flows_dir = env("JEVALAN_FLOWS_DIR")
        .map(PathBuf::from)
        .or(raw.flows_dir)
        .unwrap_or_else(|| PathBuf::from("./config/flows"));
    let capabilities_dir = env("JEVALAN_CAPABILITIES_DIR")
        .map(PathBuf::from)
        .or(raw.hub.capabilities_dir)
        .unwrap_or_else(|| Hub::default().capabilities_dir);
    let allow_exec = match env("JEVALAN_ALLOW_EXEC") {
        Some(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
        None => raw.hub.allow_exec.unwrap_or(false),
    };
    let signals_dir = env("JEVALAN_SIGNALS_DIR")
        .map(PathBuf::from)
        .or(raw.hub.signals_dir)
        .unwrap_or_else(|| Hub::default().signals_dir);
    let hub = Hub { capabilities_dir, allow_exec, signals_dir, ..Hub::default() };
    Ok(Config { addr, token, backends, routes, default_backend, flows_dir, hub })
}

/// Validates the routes against the backends and drops the cloud members of those that did not opt in.
fn build_routes(
    raw: BTreeMap<String, RawRoute>,
    backends: &BTreeMap<String, Backend>,
) -> Result<BTreeMap<String, Route>, String> {
    let mut routes = BTreeMap::new();
    for (name, route) in raw {
        if !valid_name(&name) {
            return Err(format!("nombre de ruta inválido «{name}»: use [a-z0-9-]+ y distinto de «v1»"));
        }
        if backends.contains_key(&name) {
            return Err(format!("la ruta «{name}» tiene el nombre de un backend"));
        }
        let (allow_cloud, members) = match route {
            RawRoute::Short(members) => (false, members),
            RawRoute::Long { allow_cloud, members } => (allow_cloud, members),
        };
        if members.is_empty() {
            return Err(format!("la ruta «{name}» no tiene miembros"));
        }
        for m in &members {
            if !backends.contains_key(&m.backend) {
                return Err(format!("la ruta «{name}» nombra el backend «{}», que no existe", m.backend));
            }
            if m.model.trim().is_empty() {
                return Err(format!("la ruta «{name}»: el miembro «{}» necesita un model", m.backend));
            }
        }
        let kept: Vec<Member> = members.into_iter().filter(|m| allow_cloud || !backends[&m.backend].cloud).collect();
        if kept.is_empty() {
            return Err(format!("la ruta «{name}» solo tiene miembros en la nube y no declara allow_cloud: true"));
        }
        routes.insert(name, Route { allow_cloud, members: kept });
    }
    Ok(routes)
}

/// Reads `KEY=value` lines; ignores empty ones, comments and the `export` prefix.
pub fn parse_dotenv(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        let unquoted =
            ['"', '\''].iter().find_map(|q| value.strip_prefix(*q).and_then(|v| v.strip_suffix(*q))).unwrap_or(value);
        out.insert(key.trim().to_string(), unquoted.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    fn load_yaml(yaml: &str, pairs: &[(&str, &str)]) -> Result<Config, String> {
        let raw: RawConfig = serde_yaml::from_str(yaml).map_err(|e| e.to_string())?;
        build(raw, &env_of(pairs))
    }

    const SAMPLE: &str = r#"
server: { addr: "127.0.0.1:9000", token_env: JEVALAN_TOKEN }
backends:
  typesafe: { url: "https://api.typesafe.ai/", api_key_env: TS_KEY, timeout_ms: 2000 }
  ollama:   { url: "http://localhost:11434" }
default_backend: ollama
flows_dir: ./misflujos
"#;

    #[test]
    fn loads_the_sample_yaml() {
        let cfg = load_yaml(SAMPLE, &[("TS_KEY", "secreta"), ("JEVALAN_TOKEN", "t")]).unwrap();
        assert_eq!(cfg.addr, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(cfg.token.as_deref(), Some("t"));
        assert_eq!(cfg.default_backend, "ollama");
        assert_eq!(cfg.flows_dir, PathBuf::from("./misflujos"));
        let ts = &cfg.backends["typesafe"];
        assert_eq!(ts.url, "https://api.typesafe.ai");
        assert_eq!(ts.api_key.as_deref(), Some("secreta"));
        assert_eq!(ts.timeout, Duration::from_millis(2000));
        assert_eq!(cfg.backends["ollama"].timeout, Duration::from_millis(5000));
    }

    #[test]
    fn defaults_without_a_file() {
        let cfg = load(None, &env_of(&[])).unwrap();
        assert_eq!(cfg.addr, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(cfg.default_backend, "ollama");
        assert_eq!(cfg.backends["ollama"].url, "http://localhost:11434");
        assert_eq!(cfg.flows_dir, PathBuf::from("./config/flows"));
        assert!(cfg.token.is_none());
    }

    #[test]
    fn env_overrides_addr_and_default_backend() {
        let pairs = [("TS_KEY", "k"), ("JEVALAN_ADDR", "127.0.0.1:1234"), ("JEVALAN_DEFAULT_BACKEND", "typesafe")];
        let cfg = load_yaml(SAMPLE, &pairs).unwrap();
        assert_eq!(cfg.addr, "127.0.0.1:1234".parse().unwrap());
        assert_eq!(cfg.default_backend, "typesafe");
    }

    #[test]
    fn missing_api_key_env_is_an_error() {
        let err = load_yaml(SAMPLE, &[]).unwrap_err();
        assert!(err.contains("TS_KEY"), "{err}");
        assert!(load_yaml(SAMPLE, &[("TS_KEY", "")]).is_err());
    }

    #[test]
    fn reserved_or_malformed_names_are_errors() {
        assert!(load_yaml("backends: { v1: { url: 'http://x' } }\ndefault_backend: v1", &[]).is_err());
        assert!(load_yaml("backends: { Mayus: { url: 'http://x' } }\ndefault_backend: Mayus", &[]).is_err());
    }

    #[test]
    fn backend_path_defaults_and_validates() {
        let cfg = load_yaml(SAMPLE, &[("TS_KEY", "k")]).unwrap();
        assert_eq!(cfg.backends["ollama"].path, "/v1/systemone");
        let yaml = "backends: { a: { url: 'http://x', path: /api/alpha/decisions } }\ndefault_backend: a";
        assert_eq!(load_yaml(yaml, &[]).unwrap().backends["a"].path, "/api/alpha/decisions");
        for bad in ["decisions", "/a/../b", "''"] {
            let yaml = format!("backends: {{ a: {{ url: 'http://x', path: {bad} }} }}\ndefault_backend: a");
            assert!(load_yaml(&yaml, &[]).is_err(), "{bad}");
        }
    }

    #[test]
    fn non_http_url_is_an_error() {
        assert!(load_yaml("backends: { a: { url: 'ftp://x' } }\ndefault_backend: a", &[]).is_err());
    }

    #[test]
    fn unknown_default_backend_is_an_error() {
        assert!(load_yaml("default_backend: nada", &[]).is_err());
    }

    const ROUTED: &str = r#"
backends:
  kev:  { url: "http://127.0.0.1:8009" }
  laya: { url: "http://127.0.0.1:8021" }
  nube: { url: "https://example.org", cloud: true }
default_backend: kev
"#;

    fn routed(routes: &str) -> Result<Config, String> {
        load_yaml(
            &format!(
                "{ROUTED}routes:
{routes}"
            ),
            &[],
        )
    }

    #[test]
    fn a_short_route_is_a_list_of_members() {
        let cfg = routed(
            "  local:
    - { backend: kev, model: kev-latest }
    - { backend: laya, model: laya-rl }
",
        )
        .unwrap();
        let route = &cfg.routes["local"];
        assert!(!route.allow_cloud);
        assert_eq!(route.members.iter().map(|m| m.backend.as_str()).collect::<Vec<_>>(), ["kev", "laya"]);
        assert_eq!(route.members[1].model, "laya-rl");
    }

    #[test]
    fn the_cloud_stays_out_of_a_route_that_did_not_opt_in() {
        let short = "  mixta:
    - { backend: kev, model: k }
    - { backend: nube, model: n }
";
        assert_eq!(routed(short).unwrap().routes["mixta"].members.len(), 1);
        let long = "  mixta:
    allow_cloud: true
    members:
      - { backend: kev, model: k }
      - { backend: nube, model: n }
";
        assert_eq!(routed(long).unwrap().routes["mixta"].members.len(), 2);
    }

    #[test]
    fn a_route_made_only_of_cloud_members_needs_allow_cloud() {
        let err = routed(
            "  nube-sola:
    - { backend: nube, model: n }
",
        )
        .unwrap_err();
        assert!(err.contains("allow_cloud"), "{err}");
    }

    #[test]
    fn a_route_is_checked_against_the_backends() {
        assert!(
            routed(
                "  kev:
    - { backend: laya, model: m }
"
            )
            .is_err(),
            "name clash with a backend"
        );
        assert!(
            routed(
                "  r:
    - { backend: nada, model: m }
"
            )
            .is_err(),
            "unknown member"
        );
        assert!(
            routed(
                "  r:
    - { backend: kev, model: '' }
"
            )
            .is_err(),
            "member without model"
        );
        assert!(
            routed(
                "  v1:
    - { backend: kev, model: m }
"
            )
            .is_err(),
            "reserved name"
        );
        assert!(
            routed(
                "  r: []
"
            )
            .is_err(),
            "no members"
        );
    }

    #[test]
    fn the_default_backend_may_be_a_route() {
        let yaml = format!(
            "{ROUTED}routes:
  local:
    - {{ backend: kev, model: k }}
"
        )
        .replace("default_backend: kev", "default_backend: local");
        assert_eq!(load_yaml(&yaml, &[]).unwrap().default_backend, "local");
    }

    #[test]
    fn hub_defaults_and_overrides() {
        let cfg = load_yaml("", &[]).unwrap();
        assert_eq!((cfg.hub.capabilities_dir, cfg.hub.allow_exec), (PathBuf::from("./config/capabilities"), false));
        let yaml = "hub: { capabilities_dir: ./caps, allow_exec: true }";
        let cfg = load_yaml(yaml, &[]).unwrap();
        assert_eq!((cfg.hub.capabilities_dir, cfg.hub.allow_exec), (PathBuf::from("./caps"), true));
        let pairs = [("JEVALAN_CAPABILITIES_DIR", "/otro"), ("JEVALAN_ALLOW_EXEC", "false")];
        let cfg = load_yaml(yaml, &pairs).unwrap();
        assert_eq!((cfg.hub.capabilities_dir, cfg.hub.allow_exec), (PathBuf::from("/otro"), false));
    }

    #[test]
    fn debug_hides_the_api_key() {
        let cfg = load_yaml(SAMPLE, &[("TS_KEY", "secreta")]).unwrap();
        assert!(!format!("{cfg:?}").contains("secreta"));
    }

    #[test]
    fn dotenv_handles_comments_quotes_and_export() {
        let text = "# nota\n\nA=1\nexport B=\"dos\"\nC='tres'\n  D = cuatro \nsin igual\n";
        let map = parse_dotenv(text);
        assert_eq!(map.get("A").map(String::as_str), Some("1"));
        assert_eq!(map.get("B").map(String::as_str), Some("dos"));
        assert_eq!(map.get("C").map(String::as_str), Some("tres"));
        assert_eq!(map.get("D").map(String::as_str), Some("cuatro"));
        assert_eq!(map.len(), 4);
    }

    #[test]
    fn signals_dir_por_defecto_fichero_y_entorno() {
        let none = |_: &str| None;
        assert_eq!(build(RawConfig::default(), &none).unwrap().hub.signals_dir, PathBuf::from("./config/signals"));
        let raw: RawConfig = serde_yaml::from_str("hub: { signals_dir: ./s }").unwrap();
        assert_eq!(build(raw, &none).unwrap().hub.signals_dir, PathBuf::from("./s"));
        let env = |k: &str| (k == "JEVALAN_SIGNALS_DIR").then(|| "./e".to_string());
        let raw: RawConfig = serde_yaml::from_str("hub: { signals_dir: ./s }").unwrap();
        assert_eq!(build(raw, &env).unwrap().hub.signals_dir, PathBuf::from("./e"));
    }
}
