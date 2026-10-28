//! Señales con nombre: validación (Tarea 2) y ejecución (Tarea 3).

use jevalan_flow::{Flow, FlowError, Signals, parse, parse_signal, parse_with};

const ROOT: &str = "type: rule\nwhen: 'has(input.tool_input.command) && input.tool_input.command.startsWith(\"rm\")'\n";
const BROKEN: &str = "type: rule\nwhen: 'input.nope.x == 1'\n";
const DESTRUCTIVE: &str =
    "type: noul\ninstructions: \"¿Destruye datos?\"\ncriteria: { \"true\": \"sí\", \"false\": \"no\" }\n";

fn catalog() -> Signals {
    Signals::from([
        ("root".to_string(), parse_signal("root", ROOT).unwrap()),
        ("broken".to_string(), parse_signal("broken", BROKEN).unwrap()),
        ("destructive".to_string(), parse_signal("destructive", DESTRUCTIVE).unwrap()),
    ])
}

const GUARD: &str = r#"
start: guard
nodes:
  guard: { type: rule, when: 'signals.root', out: { "true": deny, "false": allow } }
outputs:
  deny: { outcome: ok, data: { v: deny } }
  allow: { outcome: ok, data: { v: allow } }
"#;

const RISKS: &str = r#"
defaults: { backend: kev, model: kev-latest }
start: risks
nodes:
  risks:
    type: batch
    signals: [destructive]
    route:
      - { when: 'answers.risks.destructive >= 0.85', to: deny }
      - { when: 'answers.risks.destructive >= 0.55 && !signals.root', to: ask }
    else: allow
outputs:
  deny: { outcome: ok, data: { v: deny } }
  ask: { outcome: ok, data: { v: ask } }
  allow: { outcome: ok, data: { v: allow } }
"#;

fn flow(yaml: &str) -> Result<Flow, FlowError> {
    parse_with("f", yaml, &catalog(), &["kev"], "kev")
}

#[test]
fn una_heuristica_se_resuelve_y_queda_en_el_flujo() {
    let f = flow(GUARD).unwrap();
    assert_eq!(f.signals.keys().collect::<Vec<_>>(), ["root"]);
}

#[test]
fn una_aprendida_se_vuelve_pregunta_con_su_nombre() {
    let f = flow(RISKS).unwrap();
    let q = &f.nodes["risks"].questions["destructive"];
    assert_eq!((q.ty.as_str(), q.instructions.as_str()), ("noul", "¿Destruye datos?"));
    assert_eq!(f.signals.keys().collect::<Vec<_>>(), ["root"]);
}

#[test]
fn sin_catalogo_la_senal_es_desconocida() {
    let guard = parse("f", GUARD, &["kev"], "kev").unwrap_err();
    assert_eq!(guard, FlowError::UnknownSignal { node: "guard".into(), signal: "root".into() });
    assert_eq!(guard.to_string(), "guard: señal desconocida \"root\"");
    let risks = parse("f", RISKS, &["kev"], "kev").unwrap_err();
    assert_eq!(risks, FlowError::UnknownSignal { node: "risks".into(), signal: "destructive".into() });
}

#[test]
fn cada_familia_en_su_sitio() {
    // Una heurística no se pregunta…
    let preguntada = RISKS.replace("signals: [destructive]", "signals: [root]");
    assert!(matches!(flow(&preguntada), Err(FlowError::BadCriteria { node, .. }) if node == "risks.root"));
    // …y una aprendida no se lee en CEL.
    let leida = GUARD.replace("signals.root", "signals.destructive");
    assert!(matches!(flow(&leida), Err(FlowError::BadCriteria { node, .. }) if node == "guard"));
}

#[test]
fn signals_solo_vale_para_batch_y_no_pisa_preguntas() {
    let suelto = GUARD.replace("type: rule,", "type: rule, signals: [destructive],");
    assert!(matches!(flow(&suelto), Err(FlowError::BadCriteria { node, .. }) if node == "guard"));
    let doble = RISKS.replace(
        "signals: [destructive]",
        "signals: [destructive]\n    questions:\n      destructive: { type: noul, instructions: x }",
    );
    assert!(matches!(flow(&doble), Err(FlowError::BadCriteria { node, .. }) if node == "risks.destructive"));
}

#[test]
fn una_senal_dice_que_se_pregunta_y_nada_mas() {
    let mal = |name: &str, yaml: &str| match parse_signal(name, yaml) {
        Err(FlowError::BadSignal { reason, .. }) => reason,
        other => panic!("{other:?}"),
    };
    assert!(mal("Root", ROOT).contains("identificador CEL"));
    assert!(mal("root", &format!("{ROOT}backend: kev\n")).contains("solo admite"));
    assert!(mal("root", "type: rule\nwhen: 'answers.x == 1'\n").contains("solo puede leer input"));
    assert!(mal("root", "type: rule\nwhen: 'signals.otra'\n").contains("solo puede leer input"));
    assert!(mal("root", "type: rule\nwhen: '1 +'\n").contains("CEL inválida"));
    assert!(mal("root", "type: rule\n").contains("necesita when"));
    assert!(mal("b", "type: batch\n").contains("no puede ser batch"));
    assert!(mal("d", &format!("{DESTRUCTIVE}when: 'true'\n")).contains("when solo vale"));
    // C1: una aprendida sin pregunta mandaría al modelo una pregunta vacía; no se carga.
    assert!(mal("d", "type: noul\n").contains("necesita instructions"));
    assert!(mal("d", "type: noul\ninstructions: \"  \"\n").contains("necesita instructions"));
}

// --- Ejecución (Tarea 3) ---

use jevalan_flow::{Next, Run};
use serde_json::{Map, Value, json};

fn noul(p: f64) -> Map<String, Value> {
    Map::from_iter([("destructive".to_string(), json!({ "type": "noul", "noul": p, "confidence": 0.9 }))])
}

#[test]
fn un_rule_evalua_la_heuristica_y_la_deja_en_el_paso() {
    let f = flow(GUARD).unwrap();
    let mut run = Run::new(&f, json!({ "tool_input": { "command": "rm -rf /" } }));
    let Next::Ruled(step) = run.next() else { panic!("se esperaba el rule") };
    assert_eq!((step.port.as_str(), step.signals.get("root")), ("true", Some(&Value::Bool(true))));
    assert!(matches!(run.next(), Next::Outcome { outcome: "ok", data: Some(d) } if d == json!({ "v": "deny" })));
}

#[test]
fn una_heuristica_que_falla_lleva_al_puerto_de_error() {
    let yaml = GUARD
        .replace("signals.root", "signals.broken")
        .replace(r#""false": allow }"#, r#""false": allow, error: deny }"#);
    let f = flow(&yaml).unwrap();
    let mut run = Run::new(&f, json!({}));
    let Next::Ruled(step) = run.next() else { panic!("se esperaba el rule") };
    assert_eq!(step.port, "error");
    assert!(matches!(run.next(), Next::Outcome { data: Some(d), .. } if d == json!({ "v": "deny" })));
}

#[test]
fn un_batch_anota_la_regla_que_gano_y_las_heurísticas_que_leyo() {
    let f = flow(RISKS).unwrap();
    let input = json!({ "tool_input": { "command": "ls" } });

    let mut run = Run::new(&f, input.clone());
    assert!(matches!(run.next(), Next::BatchAsk { .. }));
    let step = run.answer_batch(Ok(noul(0.6)), 0).unwrap().clone();
    assert_eq!((step.port.as_str(), step.rule), ("ask", Some(1)));
    assert_eq!(step.signals, Map::from_iter([("root".to_string(), Value::Bool(false))]));

    let mut run = Run::new(&f, input);
    run.next();
    let step = run.answer_batch(Ok(noul(0.1)), 0).unwrap().clone();
    assert_eq!((step.port.as_str(), step.rule), ("allow", None));
}

#[test]
fn solo_se_evalua_la_heuristica_de_la_regla_que_se_mira() {
    // `broken` falla si se evalúa; la primera regla gana antes de llegar a ella (D4).
    let f = flow(&RISKS.replace("!signals.root", "signals.broken")).unwrap();
    let mut run = Run::new(&f, json!({}));
    run.next();
    let step = run.answer_batch(Ok(noul(0.9)), 0).unwrap().clone();
    assert_eq!((step.port.as_str(), step.rule), ("deny", Some(0)));
    assert!(step.signals.is_empty());
}

#[test]
fn un_batch_que_no_puede_evaluar_su_heuristica_falla() {
    // D5: con 0.6 se mira la segunda regla, `broken` falla, y no hay `else` que valga: nunca `allow`.
    let f = flow(&RISKS.replace("!signals.root", "signals.broken")).unwrap();
    let mut run = Run::new(&f, json!({}));
    run.next();
    let step = run.answer_batch(Ok(noul(0.6)), 0).unwrap().clone();
    assert_eq!(step.port, "error");
    // I2: el error de una heurística es de regla: el daemon publica la frase genérica, no la CEL con la entrada.
    assert!(matches!(run.next(), Next::Failed { rule: true, .. }));
}
