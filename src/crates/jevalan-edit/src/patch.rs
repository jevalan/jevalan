//! Patcher: rewrites in the original text only the touched nodes, respecting comments and order.

use jevalan_flow::NodeDef;
use std::collections::BTreeMap;
use std::ops::Range;

/// Span of lines a node occupies inside the `nodes:` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tramo {
    /// Line of the `  <node>:` header.
    pub inicio: usize,
    /// First line that no longer belongs to the node.
    pub fin: usize,
}

impl Tramo {
    pub fn rango(&self) -> Range<usize> {
        self.inicio..self.fin
    }
}

/// What has to be rewritten in the text. One patch per node, and at most one of each kind.
#[derive(Debug, Clone, PartialEq)]
pub enum Parche {
    /// Replaces the node's span with its serialised block.
    Sustituir { nombre: String, lineas: Vec<String> },
    /// Deletes the node's span, with the comments hanging from it.
    Borrar { nombre: String },
    /// Inserts the block at the end of the `nodes:` block.
    Insertar { nombre: String, lineas: Vec<String> },
}

/// Leading spaces of a line.
fn sangria(l: &str) -> usize {
    l.len() - l.trim_start_matches(' ').len()
}

fn es_comentario(l: &str) -> bool {
    l.trim_start().starts_with('#')
}

/// A root-level line of its own: no indentation and with content (a key such as `outputs:`).
fn es_raiz(l: &str) -> bool {
    !l.trim().is_empty() && !es_comentario(l) && sangria(l) == 0
}

/// Node name if the line is a header of the `nodes:` block: two spaces and a colon.
/// A node written inline (`  a: { type: noul, ... }`) is not a header: it is not recognised, and whoever
/// asks for it is left without a span rather than cutting inside a value.
fn nombre_de_nodo(l: &str) -> Option<String> {
    let resto = l.strip_prefix("  ")?;
    if resto.starts_with(' ') || es_comentario(l) {
        return None;
    }
    let (nombre, cola) = resto.split_once(':')?;
    let (nombre, cola) = (nombre.trim(), cola.trim());
    let es_encabezado = !nombre.is_empty() && (cola.is_empty() || cola.starts_with('#'));
    es_encabezado.then(|| nombre.trim_matches('"').to_string())
}

/// First line of the `nodes:` block and first one that no longer belongs to it.
fn bloque_nodes(lineas: &[&str]) -> Option<(usize, usize)> {
    let cabecera = lineas.iter().position(|l| l.trim_end() == "nodes:")?;
    let vacia_final = lineas.last().is_some_and(|l| l.is_empty());
    let fin = (cabecera + 1..lineas.len())
        .find(|i| es_raiz(lineas[*i]))
        .unwrap_or(lineas.len().saturating_sub(usize::from(vacia_final)));
    Some((cabecera + 1, fin))
}

/// Trims from the end of the span the blank lines and the comments at the header's level: they belong
/// to the next node, not this one.
fn recorta_cola(lineas: &[&str], inicio: usize, fin: usize) -> usize {
    let mut fin = fin;
    while fin > inicio {
        let anterior = lineas[fin - 1];
        if anterior.trim().is_empty() || (es_comentario(anterior) && sangria(anterior) <= 2) {
            fin -= 1;
        } else {
            break;
        }
    }
    fin
}

/// The span of each node of the `nodes:` block, in the order they appear.
pub fn tramos(lineas: &[&str]) -> Vec<(String, Tramo)> {
    let Some((inicio, fin)) = bloque_nodes(lineas) else { return Vec::new() };
    let cabeceras: Vec<(usize, String)> =
        (inicio..fin).filter_map(|i| nombre_de_nodo(lineas[i]).map(|n| (i, n))).collect();
    cabeceras
        .iter()
        .enumerate()
        .map(|(k, (i, nombre))| {
            let crudo = cabeceras.get(k + 1).map_or(fin, |(j, _)| *j);
            (nombre.clone(), Tramo { inicio: *i, fin: recorta_cola(lineas, *i + 1, crudo) })
        })
        .collect()
}

/// A node's YAML block, already indented: the first line is `  <name>:`.
/// `serde_yaml` does not wrap lines (the emitter has no width), so a long `instructions`
/// ends up on a single line; it is the price of rewriting the node with the serialiser.
pub fn bloque(nombre: &str, def: &NodeDef) -> Vec<String> {
    let cuerpo = serde_yaml::to_string(def).unwrap_or_default();
    let mut lineas = vec![format!("  {nombre}:")];
    lineas.extend(cuerpo.split('\n').filter(|l| !l.is_empty()).map(|l| format!("    {l}")));
    lineas
}

/// Applies the patches to the original text; returns the new text and the names that could not
/// be touched (a node the text does not write as a block, for instance).
pub fn aplicar(texto: &str, parches: &[Parche]) -> (String, Vec<String>) {
    let mut lineas: Vec<String> = texto.split('\n').map(str::to_string).collect();
    let tramos: BTreeMap<String, Tramo> = {
        let vista: Vec<&str> = lineas.iter().map(String::as_str).collect();
        tramos(&vista).into_iter().collect()
    };
    // The file keeps its line ending: inserted lines carry the same one.
    let cr = if texto.contains("\r\n") { "\r" } else { "" };
    let con_final = |ls: &[String]| -> Vec<String> { ls.iter().map(|l| format!("{l}{cr}")).collect() };

    let mut cambios: Vec<(Tramo, Vec<String>)> = Vec::new();
    let mut borrados: Vec<Tramo> = Vec::new();
    let mut insertados: Vec<(String, Vec<String>)> = Vec::new();
    let mut perdidos: Vec<String> = Vec::new();
    for parche in parches {
        match parche {
            Parche::Sustituir { nombre, lineas: bloque } => match tramos.get(nombre) {
                Some(tramo) => cambios.push((tramo.clone(), con_final(bloque))),
                None => perdidos.push(nombre.clone()),
            },
            Parche::Borrar { nombre } => match tramos.get(nombre) {
                Some(tramo) => borrados.push(tramo.clone()),
                None => perdidos.push(nombre.clone()),
            },
            Parche::Insertar { nombre, lineas: bloque } => insertados.push((nombre.clone(), con_final(bloque))),
        }
    }
    // Bottom-up: this way the indices of the spans still to be touched do not move. Replacements
    // and deletions go in **a single** pass, sorted together: applying them in two
    // rounds would leave the second one's indices measured against the old text, and replacing a node that
    // comes before the one being deleted (if its block changes length) would misplace the deletion.
    let mut tocados: Vec<(Tramo, Vec<String>)> = cambios;
    tocados.extend(borrados.into_iter().map(|tramo| (tramo, Vec::new())));
    tocados.sort_by_key(|(tramo, _)| std::cmp::Reverse(tramo.inicio));
    for (tramo, bloque) in tocados {
        lineas.splice(tramo.rango(), bloque);
    }
    if !insertados.is_empty() {
        let vistas: Vec<String> = insertados.iter().flat_map(|(_, bloque)| bloque.clone()).collect();
        let indice = {
            let vista: Vec<&str> = lineas.iter().map(String::as_str).collect();
            bloque_nodes(&vista).map(|(_, fin)| fin)
        };
        match indice {
            Some(fin) => {
                lineas.splice(fin..fin, vistas);
            }
            // Without a `nodes:` block there is nowhere to insert: better to say so than to write somewhere else.
            None => perdidos.extend(insertados.into_iter().map(|(nombre, _)| nombre)),
        }
    }
    (lineas.join("\n"), perdidos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevalan_flow::{Criteria, FlowDef, NodeDef, parse, validate};

    /// The real flow, with its calibration comments, as working text.
    const LOCAL: &str = include_str!("../../jevalan-flow/fixtures/tool-risk-local-v1.yaml");
    const BACKENDS: [&str; 1] = ["kev"];

    fn def(yaml: &str) -> FlowDef {
        serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{e}"))
    }

    /// The patch `Documento` builds for a touched node.
    fn parche(nombre: &str, d: &NodeDef) -> Parche {
        Parche::Sustituir { nombre: nombre.into(), lineas: bloque(nombre, d) }
    }

    #[test]
    fn los_comentarios_y_el_orden_sobreviven_al_parche() {
        let mut d = def(LOCAL);
        let when = d.nodes["risks"].route[1].when.replace("0.55", "0.6");
        d.nodes.get_mut("risks").unwrap().route[1].when = when;
        let nuevo = aplicar(LOCAL, &[parche("risks", &d.nodes["risks"])]).0;

        // What D1 protects: the hand-written header.
        assert!(nuevo.contains("# tool.risk servido en local (Kev o Laya)"), "{nuevo}");
        assert!(nuevo.contains("# Réplica sobria de Toolgate"), "{nuevo}");
        assert!(nuevo.contains("0.55 → ask, 0.85 → deny"), "la calibración del comentario sigue: {nuevo}");
        // The change is there, and the old value is no longer hard-coded.
        assert!(nuevo.contains(">= 0.6"), "{nuevo}");
        assert!(!nuevo.contains(">= 0.55"), "{nuevo}");
        // Untouched nodes stay byte for byte, with their odd quotes.
        assert!(
            nuevo.contains(r#"has(input.tool_input.command) && input.tool_input.command.matches(r"rm\s+-"#),
            "{nuevo}"
        );
        assert!(nuevo.contains("source: fail-mode"), "las salidas siguen: {nuevo}");
        // D1's limit, written down so nobody is misled: the comments inside the
        // touched node go with it, because its whole span is replaced.
        assert!(!nuevo.contains("# Gana la primera que sea verdadera"), "{nuevo}");

        // And the text parses again into the same `Flow` as the `FlowDef` in progress, which is D1's proof.
        let leido = parse("tool-risk-local", &nuevo, &BACKENDS, "kev").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(leido, validate("tool-risk-local", d, &BACKENDS, "kev").unwrap());
    }

    #[test]
    fn el_texto_parcheado_conserva_crlf() {
        let crlf = LOCAL.replace('\n', "\r\n");
        let mut d = def(&crlf);
        d.nodes.get_mut("risks").unwrap().questions.get_mut("privilege").unwrap().instructions = "Otra pregunta".into();
        let nuevo = aplicar(&crlf, &[parche("risks", &d.nodes["risks"])]).0;

        // No stray line: the file is still CRLF from first to last.
        assert!(nuevo.split('\n').filter(|l| !l.is_empty()).all(|l| l.ends_with('\r')), "{nuevo}");
        let leido: FlowDef = serde_yaml::from_str(&nuevo).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(leido, d);
    }

    #[test]
    fn un_nodo_en_linea_no_se_parchea_a_ciegas() {
        let texto = "start: a\nnodes:\n  a: { type: noul, instructions: \"¿Rompe algo?\", out: { \"true\": b, \"false\": b } }\noutputs:\n  b: { outcome: ok }\n";
        let d = def(texto);
        let (nuevo, perdidos) = aplicar(texto, &[parche("a", &d.nodes["a"])]);
        assert_eq!(perdidos, ["a"], "un nodo en línea no se reconoce, y se dice cuál");
        assert_eq!(nuevo, texto, "y el texto queda intacto: {nuevo}");
    }

    #[test]
    fn solo_se_reescribe_el_nodo_nombrado() {
        let texto = "# los risks viejos, ya no se usan\nstart: risk\nnodes:\n  risk:\n    type: noul\n    instructions: \"x\"\n    out: { \"true\": risks, \"false\": risks }\n  # ojo: risks es el nodo delicado\n  risks:\n    type: noul\n    instructions: \"y\"\n    out: { \"true\": allow, \"false\": allow }\noutputs:\n  allow: { outcome: ok }\n";
        let mut d = def(texto);
        d.nodes.get_mut("risks").unwrap().instructions = "y, editada".into();
        let nuevo = aplicar(texto, &[parche("risks", &d.nodes["risks"])]).0;

        // The comment that names `risks` is still where it was, and `risk` has not been touched.
        assert!(nuevo.contains("# los risks viejos, ya no se usan"), "{nuevo}");
        assert!(
            nuevo.contains("  # ojo: risks es el nodo delicado"),
            "el comentario del nodo vecino se queda con él: {nuevo}"
        );
        assert!(nuevo.contains("    instructions: \"x\""), "el otro nodo, intacto: {nuevo}");
        assert!(nuevo.contains("y, editada"), "{nuevo}");
        let leido: FlowDef = serde_yaml::from_str(&nuevo).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(leido, d);
    }

    #[test]
    fn sustituir_arriba_y_borrar_abajo_no_descoloca_los_tramos() {
        // The replaced node grows: if the deletion were applied afterwards with indices from the old text,
        // it would land on the wrong lines. It is the "rewire above and delete below" case.
        let texto = "start: a\nnodes:\n  a:\n    type: noul\n    instructions: \"x\"\n    out: { \"true\": b, \"false\": b }\n  b:\n    type: noul\n    instructions: \"y\"\n    out: { \"true\": ok, \"false\": ok }\noutputs:\n  ok: { outcome: ok }\n";
        let d = def(texto);
        let mut crecido = d.nodes["a"].clone();
        crecido.instructions = "x, ahora con una instrucción bastante más larga que antes".into();
        crecido.criteria = Some(Criteria::Options(
            [("true".to_string(), Some("Sí".to_string())), ("false".to_string(), Some("No".to_string()))].into(),
        ));
        let (nuevo, perdidos) = aplicar(
            texto,
            &[
                Parche::Sustituir { nombre: "a".into(), lineas: bloque("a", &crecido) },
                Parche::Borrar { nombre: "b".into() },
            ],
        );

        assert!(perdidos.is_empty(), "{perdidos:?}");
        assert!(!nuevo.contains("  b:"), "el borrado se lleva su tramo entero: {nuevo}");
        assert!(nuevo.contains("bastante más larga"), "{nuevo}");
        // And the text is still the intended flow: `a` grown, `b` gone.
        let mut esperado = d.clone();
        esperado.nodes.insert("a".into(), crecido);
        esperado.nodes.remove("b");
        let leido: FlowDef = serde_yaml::from_str(&nuevo).unwrap_or_else(|e| panic!("{e}\n{nuevo}"));
        assert_eq!(leido, esperado, "{nuevo}");
    }

    #[test]
    fn los_nodos_nuevos_van_tras_el_ultimo_y_los_borrados_desaparecen() {
        let texto = "start: a\nnodes:\n  a:\n    type: noul\n    instructions: \"x\"\n    out: { \"true\": b, \"false\": b }\n  b:\n    type: noul\n    instructions: \"y\"\n    out: { \"true\": ok, \"false\": ok }\noutputs:\n  ok: { outcome: ok }\n";
        let d = def(texto);
        let mut nuevo_b = d.nodes["b"].clone();
        nuevo_b.instructions = "y, la nueva".into();
        let (nuevo, perdidos) = aplicar(
            texto,
            &[
                Parche::Borrar { nombre: "a".into() },
                Parche::Insertar { nombre: "c".into(), lineas: bloque("c", &nuevo_b) },
            ],
        );

        assert!(perdidos.is_empty(), "{perdidos:?}");
        assert!(!nuevo.contains("  a:"), "el borrado se lleva su tramo: {nuevo}");
        let (pos_c, pos_salidas) = (nuevo.find("  c:").unwrap(), nuevo.find("outputs:").unwrap());
        assert!(nuevo.find("  b:").unwrap() < pos_c && pos_c < pos_salidas, "el nuevo va tras el último nodo: {nuevo}");
        let leido: FlowDef = serde_yaml::from_str(&nuevo).unwrap();
        assert!(leido.nodes.contains_key("c") && !leido.nodes.contains_key("a"));
    }
}
