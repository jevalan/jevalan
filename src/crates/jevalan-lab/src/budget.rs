//! Presupuesto de `record` y `live`: cota previa, confirmación y cuenta corriente.

use jevalan_flow::{Flow, NodeType};
use jevalan_hub::lab::config::Budget;
use std::collections::HashMap;
use std::io::{BufRead, Write};

/// Nodos Jev del camino más largo del flujo: cota superior de las llamadas de una ejecución.
pub fn longest_path(flow: &Flow) -> u64 {
    // Memoizado por nodo: los flujos validados son acíclicos, pero un ciclo valdría 0 en vez de colgarse.
    fn walk(flow: &Flow, at: &str, memo: &mut HashMap<String, u64>) -> u64 {
        if let Some(&n) = memo.get(at) {
            return n;
        }
        let Some(node) = flow.nodes.get(at) else { return 0 };
        memo.insert(at.to_string(), 0);
        let below = node.out.values().map(|to| walk(flow, to, memo)).max().unwrap_or(0);
        let n = below + u64::from(node.ty != NodeType::Rule);
        memo.insert(at.to_string(), n);
        n
    }
    walk(flow, &flow.start, &mut HashMap::new())
}

/// Muestra el mensaje y lee una línea; solo `s`, `si`, `sí`, `y` o `yes` confirman.
pub fn ask(message: &str, input: &mut impl BufRead) -> bool {
    print!("{message}\n¿Continuar? [s/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    input.read_line(&mut line).is_ok() && matches!(line.trim().to_lowercase().as_str(), "s" | "si" | "sí" | "y" | "yes")
}

/// Llamadas y coste conocido consumidos hasta ahora, más el mayor coste por llamada observado.
pub struct Tally {
    budget: Budget,
    pub calls: u64,
    pub cost: f64,
    per_call: f64,
}

impl Tally {
    pub fn new(budget: Budget) -> Self {
        Self { budget, calls: 0, cost: 0.0, per_call: 0.0 }
    }

    /// Anota una ejecución: `planned` llamadas previstas, `made` hechas de verdad y su coste conocido.
    pub fn record(&mut self, planned: u64, made: u32, usd: Option<f64>) {
        self.calls += planned;
        if let Some(usd) = usd {
            self.cost += usd;
            if made > 0 {
                self.per_call = self.per_call.max(usd / f64::from(made));
            }
        }
    }

    /// ¿Ejecutar `next` llamadas más rompería el presupuesto? Si `priced`, también se proyecta el gasto con el
    /// mayor coste por llamada visto; sin ninguna observación previa la primera pasa y no se puede proyectar.
    pub fn exhausted(&self, next: u64, priced: bool) -> bool {
        let projected = if priced { next as f64 * self.per_call } else { 0.0 };
        self.calls + next > self.budget.max_calls || self.cost + projected > self.budget.max_cost_usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(max_cost_usd: f64) -> Budget {
        Budget { max_calls: 100, max_cost_usd }
    }

    #[test]
    fn projects_the_largest_cost_per_call() {
        let mut t = Tally::new(budget(1.0));
        assert!(!t.exhausted(2, true), "sin observaciones, la primera pasa");
        t.record(2, 2, Some(0.4));
        assert!(!t.exhausted(2, true), "0,4 + 2 x 0,2 cabe justo");
        t.record(1, 1, Some(0.3));
        assert!(t.exhausted(2, true), "0,7 + 2 x 0,3 se pasa");
        assert!(!t.exhausted(2, false), "lo que no tiene precio no proyecta gasto");
    }

    #[test]
    fn longest_path_counts_jev_nodes() {
        let yaml = include_str!("../../../../config/flows/agent-wakeup.yaml");
        let flow = jevalan_flow::parse("agent-wakeup", yaml, &["openrouter"], "openrouter").unwrap();
        assert_eq!(longest_path(&flow), 1);
    }

    /// Un nodo `batch` es **una** llamada, por muchas preguntas que lleve (D6).
    #[test]
    fn longest_path_counts_a_batch_node_once() {
        let yaml = r#"
defaults: { backend: ollama, model: nimble }
start: risks
nodes:
  risks:
    type: batch
    questions:
      destructive: { type: noul, instructions: "x" }
      exfiltration: { type: noul, instructions: "x" }
      privilege: { type: noul, instructions: "x" }
      secret_exposure: { type: noul, instructions: "x" }
    route:
      - { when: "answers.risks.destructive >= 0.85", to: deny }
    else: allow
outputs:
  allow: { outcome: allow }
  deny: { outcome: deny }
"#;
        let flow = jevalan_flow::parse("tool-risk", yaml, &["ollama"], "ollama").unwrap();
        assert_eq!(longest_path(&flow), 1);
    }
}
