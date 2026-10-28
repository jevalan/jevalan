//! Parcheador: reescribe en el texto original solo los nodos tocados, respetando comentarios y orden.

use jevalan_flow::NodeDef;
use std::collections::BTreeMap;
use std::ops::Range;

/// Tramo de líneas que ocupa un nodo dentro del bloque `nodes:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tramo {
    /// Línea del encabezado `  <nodo>:`.
    pub inicio: usize,
    /// Primera línea que ya no pertenece al nodo.
    pub fin: usize,
}

impl Tramo {
    pub fn rango(&self) -> Range<usize> {
        self.inicio..self.fin
    }
}

/// Lo que hay que reescribir en el texto. Un parche por nodo, y como mucho uno de cada clase.
#[derive(Debug, Clone, PartialEq)]
pub enum Parche {
    /// Sustituye el tramo del nodo por su bloque serializado.
    Sustituir { nombre: String, lineas: Vec<String> },
    /// Borra el tramo del nodo, con los comentarios que cuelgan de él.
    Borrar { nombre: String },
    /// Inserta el bloque al final del bloque `nodes:`.
    Insertar { nombre: String, lineas: Vec<String> },
}

/// Espacios iniciales de una línea.
fn sangria(l: &str) -> usize {
    l.len() - l.trim_start_matches(' ').len()
}

fn es_comentario(l: &str) -> bool {
    l.trim_start().starts_with('#')
}

/// Línea propia del nivel raíz: sin sangría y con contenido (una clave como `outputs:`).
fn es_raiz(l: &str) -> bool {
    !l.trim().is_empty() && !es_comentario(l) && sangria(l) == 0
}

/// Nombre del nodo si la línea es un encabezado del bloque `nodes:`: dos espacios y dos puntos.
/// Un nodo escrito en línea (`  a: { type: noul, ... }`) no es un encabezado: no se reconoce, y el
/// que lo pida se quedará sin tramo antes que cortar dentro de un valor.
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

/// Primera línea del bloque `nodes:` y primera que ya no le pertenece.
fn bloque_nodes(lineas: &[&str]) -> Option<(usize, usize)> {
    let cabecera = lineas.iter().position(|l| l.trim_end() == "nodes:")?;
    let vacia_final = lineas.last().is_some_and(|l| l.is_empty());
    let fin = (cabecera + 1..lineas.len())
        .find(|i| es_raiz(lineas[*i]))
        .unwrap_or(lineas.len().saturating_sub(usize::from(vacia_final)));
    Some((cabecera + 1, fin))
}

/// Recorta del final del tramo las líneas en blanco y los comentarios del nivel del encabezado: son
/// del nodo siguiente, no de este.
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

/// El tramo de cada nodo del bloque `nodes:`, en el orden en que aparecen.
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

/// El bloque YAML de un nodo, ya sangrado: la primera línea es `  <nombre>:`.
/// `serde_yaml` no parte las líneas (el emisor va sin anchura), así que una `instructions` larga
/// queda en una sola línea; es el precio de reescribir el nodo con el serializador.
pub fn bloque(nombre: &str, def: &NodeDef) -> Vec<String> {
    let cuerpo = serde_yaml::to_string(def).unwrap_or_default();
    let mut lineas = vec![format!("  {nombre}:")];
    lineas.extend(cuerpo.split('\n').filter(|l| !l.is_empty()).map(|l| format!("    {l}")));
    lineas
}

/// Aplica los parches al texto original; devuelve el texto nuevo y los nombres que no se pudieron
/// tocar (un nodo que el texto no escribe en forma de bloque, por ejemplo).
pub fn aplicar(texto: &str, parches: &[Parche]) -> (String, Vec<String>) {
    let mut lineas: Vec<String> = texto.split('\n').map(str::to_string).collect();
    let tramos: BTreeMap<String, Tramo> = {
        let vista: Vec<&str> = lineas.iter().map(String::as_str).collect();
        tramos(&vista).into_iter().collect()
    };
    // El fichero conserva su final de línea: las líneas que se insertan llevan el mismo.
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
    // De abajo arriba: así los índices de los tramos que quedan por tocar no se mueven. Las
    // sustituciones y los borrados van en **una sola** pasada y ordenados juntos: aplicarlos en dos
    // tandas dejaría los índices de la segunda medidos sobre el texto viejo, y sustituir un nodo que
    // va antes del que se borra —si su bloque cambia de líneas— descolocaría el borrado.
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
            // Sin bloque `nodes:` no hay dónde insertar: mejor decirlo que escribir en otro sitio.
            None => perdidos.extend(insertados.into_iter().map(|(nombre, _)| nombre)),
        }
    }
    (lineas.join("\n"), perdidos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jevalan_flow::{Criteria, FlowDef, NodeDef, parse, validate};

    /// El flujo real, con sus comentarios de calibración, como texto de trabajo.
    const LOCAL: &str = include_str!("../../jevalan-flow/fixtures/tool-risk-local-v1.yaml");
    const BACKENDS: [&str; 1] = ["kev"];

    fn def(yaml: &str) -> FlowDef {
        serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{e}"))
    }

    /// El parche que arma `Documento` para un nodo tocado.
    fn parche(nombre: &str, d: &NodeDef) -> Parche {
        Parche::Sustituir { nombre: nombre.into(), lineas: bloque(nombre, d) }
    }

    #[test]
    fn los_comentarios_y_el_orden_sobreviven_al_parche() {
        let mut d = def(LOCAL);
        let when = d.nodes["risks"].route[1].when.replace("0.55", "0.6");
        d.nodes.get_mut("risks").unwrap().route[1].when = when;
        let nuevo = aplicar(LOCAL, &[parche("risks", &d.nodes["risks"])]).0;

        // Lo que D1 protege: la cabecera escrita a mano.
        assert!(nuevo.contains("# tool.risk servido en local (Kev o Laya)"), "{nuevo}");
        assert!(nuevo.contains("# Réplica sobria de Toolgate"), "{nuevo}");
        assert!(nuevo.contains("0.55 → ask, 0.85 → deny"), "la calibración del comentario sigue: {nuevo}");
        // El cambio está, y el valor viejo ya no se codea.
        assert!(nuevo.contains(">= 0.6"), "{nuevo}");
        assert!(!nuevo.contains(">= 0.55"), "{nuevo}");
        // Los nodos que no se tocan quedan byte a byte, con sus comillas raras.
        assert!(
            nuevo.contains(r#"has(input.tool_input.command) && input.tool_input.command.matches(r"rm\s+-"#),
            "{nuevo}"
        );
        assert!(nuevo.contains("source: fail-mode"), "las salidas siguen: {nuevo}");
        // El límite de D1, escrito para que nadie se llame a engaño: los comentarios de dentro del
        // nodo tocado se van con él, porque su tramo entero se sustituye.
        assert!(!nuevo.contains("# Gana la primera que sea verdadera"), "{nuevo}");

        // Y el texto vuelve a parsear al mismo `Flow` que el `FlowDef` en curso, que es la prueba de D1.
        let leido = parse("tool-risk-local", &nuevo, &BACKENDS, "kev").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(leido, validate("tool-risk-local", d, &BACKENDS, "kev").unwrap());
    }

    #[test]
    fn el_texto_parcheado_conserva_crlf() {
        let crlf = LOCAL.replace('\n', "\r\n");
        let mut d = def(&crlf);
        d.nodes.get_mut("risks").unwrap().questions.get_mut("privilege").unwrap().instructions = "Otra pregunta".into();
        let nuevo = aplicar(&crlf, &[parche("risks", &d.nodes["risks"])]).0;

        // Ninguna línea suelta: el fichero sigue siendo CRLF de la primera a la última.
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

        // El comentario que nombra `risks` sigue donde estaba, y `risk` no se ha rozado.
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
        // El nodo sustituido crece: si el borrado se aplicara después con índices del texto viejo,
        // caería sobre las líneas equivocadas. Es el caso «recablear arriba y borrar abajo».
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
        // Y el texto sigue siendo el flujo que se pretendía: `a` crecido, `b` fuera.
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
