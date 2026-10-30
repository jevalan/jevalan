//! A node may name a route where it would name a backend; the route's members bring their own model.

use jevalan_flow::{FlowError, Signals, parse_routed};

const FLOW: &str = r#"
description: ruta
defaults: { backend: local }
start: q
nodes:
  q: { type: noul, instructions: "¿Es destructivo?", out: { "true": no, "false": yes, error: no } }
outputs:
  yes: { outcome: allow }
  no: { outcome: deny }
"#;

fn parse(yaml: &str, backends: &[&str], routes: &[&str]) -> Result<jevalan_flow::Flow, FlowError> {
    parse_routed("f", yaml, &Signals::new(), backends, routes, "kev")
}

#[test]
fn a_route_needs_no_model() {
    let flow = parse(FLOW, &["kev", "laya"], &["local"]).unwrap();
    assert_eq!(flow.nodes["q"].backend, "local");
    assert_eq!(flow.nodes["q"].model, "");
}

#[test]
fn a_route_keeps_a_model_the_flow_declares() {
    let yaml = FLOW.replace("defaults: { backend: local }", "defaults: { backend: local, model: kev-latest }");
    let flow = parse(&yaml, &["kev"], &["local"]).unwrap();
    assert_eq!(flow.nodes["q"].model, "kev-latest");
}

#[test]
fn a_backend_still_needs_its_model() {
    let yaml = FLOW.replace("backend: local", "backend: kev");
    assert!(matches!(parse(&yaml, &["kev"], &["local"]), Err(FlowError::MissingModel(_))));
}

#[test]
fn an_unknown_name_is_neither_backend_nor_route() {
    assert!(matches!(parse(FLOW, &["kev"], &[]), Err(FlowError::UnknownBackend { .. })));
}
