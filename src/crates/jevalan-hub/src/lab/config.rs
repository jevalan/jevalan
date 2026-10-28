use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabConfig {
    pub targets: BTreeMap<String, Target>,
    #[serde(default)]
    pub budget: Option<Budget>,
    #[serde(default = "one")]
    pub repeat: u32,
    #[serde(default)]
    pub prices: BTreeMap<String, Price>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub backend: String,
    pub model: String,
    #[serde(default)]
    pub local: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub max_calls: u64,
    pub max_cost_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

pub fn parse_lab_config(yaml: &str) -> Result<LabConfig, String> {
    let c: LabConfig = serde_yaml::from_str(yaml).map_err(|e| format!("lab.yaml inválido: {e}"))?;
    if c.targets.is_empty() {
        return Err("lab.yaml necesita al menos un target".into());
    }
    if let Some(bad) = c
        .targets
        .keys()
        .find(|k| k.is_empty() || !k.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'))
    {
        return Err(format!("nombre de target inválido: {bad:?}"));
    }
    if c.repeat == 0 {
        return Err("repeat debe ser al menos 1".into());
    }
    if let Some(b) = c.budget
        && (b.max_calls == 0 || b.max_cost_usd.is_nan() || b.max_cost_usd < 0.0)
    {
        return Err("budget necesita max_calls > 0 y max_cost_usd ≥ 0".into());
    }
    if c.prices.values().any(|p| !(p.input_per_mtok >= 0.0 && p.output_per_mtok >= 0.0)) {
        return Err("los precios no pueden ser negativos".into());
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
targets:
  jev: { backend: typesafe, model: jev-latest }
  nimble: { backend: ollama, model: nimble, local: true }
budget: { max_calls: 200, max_cost_usd: 1.0 }
repeat: 3
prices:
  jev-latest: { input_per_mtok: 1.0, output_per_mtok: 2.0 }
"#;

    #[test]
    fn parses_the_matrix() {
        let c = parse_lab_config(OK).unwrap();
        assert_eq!(c.targets.len(), 2);
        assert!(c.targets["nimble"].local);
        assert_eq!(c.repeat, 3);
        assert_eq!(c.budget.unwrap().max_calls, 200);
        assert_eq!(c.prices["jev-latest"].output_per_mtok, 2.0);
    }

    #[test]
    fn repeat_defaults_to_one_and_budget_is_optional() {
        let c = parse_lab_config("targets: { jev: { backend: typesafe, model: jev-latest } }").unwrap();
        assert_eq!((c.repeat, c.budget.is_none()), (1, true));
    }

    #[test]
    fn invalid_matrices_are_rejected() {
        assert!(parse_lab_config("targets: {}").is_err());
        assert!(parse_lab_config(&OK.replace("repeat: 3", "repeat: 0")).is_err());
        assert!(parse_lab_config(&OK.replace("nimble:", "Nimble X:")).is_err());
        assert!(parse_lab_config(&OK.replace("max_cost_usd: 1.0", "max_cost_usd: -1.0")).is_err());
        assert!(parse_lab_config(&OK.replace("max_calls: 200", "max_calls: 0")).is_err());
    }
}
