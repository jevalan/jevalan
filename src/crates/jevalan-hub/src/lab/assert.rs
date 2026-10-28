use crate::lab::fixture::Fixture;
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub why: String,
    /// Motivo cuando la expresión no pudo evaluarse o no devolvió un booleano.
    pub reason: Option<String>,
}

/// Aserciones no cumplidas. Una expresión que no se puede evaluar cuenta como fallida.
pub fn assert(f: &Fixture, response: &Value) -> Vec<Failure> {
    let section = |k: &str| response.get(k).filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
    let status = response.get("status").cloned().unwrap_or(Value::Null);
    let mut ctx = cel::Context::default();
    let vars = [
        ("status", status),
        ("decision", section("decision")),
        ("abstain", section("abstain")),
        ("error", section("error")),
        ("trace", section("trace")),
    ];
    for (name, value) in vars {
        if let Err(e) = ctx.add_variable(name, value) {
            return f.expect.iter().map(|x| Failure { why: x.why.clone(), reason: Some(e.to_string()) }).collect();
        }
    }
    f.expect
        .iter()
        .filter_map(|x| {
            let fail = |reason| Some(Failure { why: x.why.clone(), reason });
            match crate::program::compiled(&x.cel).map(|p| p.execute(&ctx)) {
                Ok(Ok(cel::Value::Bool(true))) => None,
                Ok(Ok(cel::Value::Bool(false))) => fail(None),
                Ok(Ok(other)) => fail(Some(format!("no devuelve un booleano: {other:?}"))),
                Ok(Err(e)) => fail(Some(e.to_string())),
                Err(e) => fail(Some(e.to_string())),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab::fixture::parse_fixture;
    use serde_json::json;

    fn fixture(expect: &str) -> Fixture {
        let y = format!("id: f\nversion: 1\ninput: {{}}\nexpect:\n{expect}");
        parse_fixture("f", &y).unwrap()
    }

    #[test]
    fn passing_assertions_return_nothing() {
        let f = fixture("  - { cel: \"status == 'ok' && decision.verdict == 'deny'\", why: w }\n");
        assert!(assert(&f, &json!({"status": "ok", "decision": {"verdict": "deny"}})).is_empty());
    }

    #[test]
    fn false_assertion_fails_with_its_why() {
        let f = fixture("  - { cel: \"decision.verdict != 'allow'\", why: \"nunca allow\" }\n");
        let got = assert(&f, &json!({"status": "ok", "decision": {"verdict": "allow"}}));
        assert_eq!(got, vec![Failure { why: "nunca allow".into(), reason: None }]);
    }

    #[test]
    fn unevaluable_assertion_fails_with_reason() {
        let f = fixture("  - { cel: \"decision.verdict == 'deny'\", why: w }\n");
        let got = assert(&f, &json!({"status": "abstain", "abstain": {}}));
        assert_eq!(got.len(), 1);
        assert!(got[0].reason.is_some());
    }

    #[test]
    fn non_boolean_assertion_fails() {
        let f = fixture("  - { cel: \"status\", why: w }\n");
        assert_eq!(assert(&f, &json!({"status": "ok"})).len(), 1);
    }

    #[test]
    fn missing_sections_are_empty_objects() {
        let f = fixture("  - { cel: \"size(error) == 0 && size(decision) == 0\", why: w }\n");
        assert!(assert(&f, &json!({"status": "abstain", "abstain": {"reason": null}})).is_empty());
    }
}
