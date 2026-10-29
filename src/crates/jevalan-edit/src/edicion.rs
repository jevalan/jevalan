//! Draft: the editing operations over the `FlowDef` in progress and its validation with the real parser.

use crate::daemon::Cliente;
use crate::documento::Documento;
use crate::inspector::Campo;
use jevalan_flow::{Criteria, FlowError, NodeDef, NodeType, RouteDef, Signals, parse_with};
use std::collections::BTreeMap;
use std::path::Path;

/// The types the engine already supports, and therefore the ones the editor can add.
pub const TIPOS: [NodeType; 5] = [NodeType::Rule, NodeType::Noul, NodeType::Choice, NodeType::Score, NodeType::Batch];

/// The verdict of the validation in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct Validez {
    pub ok: bool,
    /// The real `FlowError` message, as is.
    pub error: Option<String>,
    /// Node the parser points at, if the error names one.
    pub nodo: Option<String>,
}

impl Validez {
    fn valido() -> Self {
        Self { ok: true, error: None, nodo: None }
    }

    fn invalido(error: String, nodo: Option<String>) -> Self {
        Self { ok: false, error: Some(error), nodo }
    }
}

/// The draft: the document and the parser's verdict on it.
pub struct Borrador {
    pub doc: Documento,
    /// The daemon's backend catalogue, which arrives over the bus; without it, local validation cannot
    /// judge the backends and will say so.
    backends: Vec<String>,
    backend_por_defecto: String,
    /// Signal catalogue: without it, a flow that uses them cannot be judged locally.
    senales: Signals,
    /// Daemon client; without it, validation is the local parser's.
    cliente: Option<Cliente>,
    pub validez: Validez,
    /// Band notice for when the daemon does not answer and validation was done locally.
    pub aviso_validacion: Option<String>,
}

impl Borrador {
    pub fn nuevo(doc: Documento) -> Self {
        Self {
            doc,
            backends: Vec::new(),
            backend_por_defecto: String::new(),
            senales: Signals::new(),
            cliente: None,
            validez: Validez::valido(),
            aviso_validacion: None,
        }
    }

    /// The backend catalogue the daemon declared over the bus.
    pub fn catalogo(&mut self, backends: Vec<String>, por_defecto: String) {
        self.backends = backends;
        self.backend_por_defecto = por_defecto;
    }

    /// The signal catalogue against which the draft is judged locally.
    pub fn senales(&mut self, senales: Signals) {
        self.senales = senales;
    }

    /// The flow's name, which is the file stem.
    pub fn nombre(&self) -> String {
        self.doc.nombre()
    }

    /// The daemon client: with it, the verdict is the daemon's; without it, the local parser's.
    pub fn con_daemon(&mut self, cliente: Option<Cliente>) {
        self.cliente = cliente;
    }

    /// Revalidates the draft: the daemon's verdict if it answers, otherwise the local parser's (D6).
    pub fn revisar(&mut self) {
        // An unreadable document is sent to nobody: it is already known not to be valid.
        if let Some(razon) = self.doc.solo_lectura() {
            self.validez = Validez::invalido(format!("el YAML no se pudo leer: {razon}"), None);
            return;
        }
        let yaml = self.doc.yaml();
        let local = self.juzgar();
        match self.cliente.as_ref().map(|c| c.validar(&self.nombre(), &yaml)) {
            // The daemon is the authority: its message rules. The guilty node is still pointed out by the
            // local parser, which is exactly the one it uses.
            Some(Ok(veredicto)) => {
                self.aviso_validacion = None;
                self.validez = match (veredicto.ok, veredicto.error) {
                    (true, _) => Validez::valido(),
                    (false, error) => Validez::invalido(error.unwrap_or_default(), local.nodo),
                };
            }
            // The daemon does not answer: carry on with the local parser, and say so in the bottom band.
            Some(Err(aviso)) => {
                self.aviso_validacion = Some(format!("{aviso}; validación local{}", self.aviso_backends()));
                self.validez = local;
            }
            None => {
                self.aviso_validacion = self.aviso_backends().strip_prefix("; ").map(str::to_string);
                self.validez = local;
            }
        }
    }

    /// The notice that accompanies a local validation done **without** the daemon's catalogue.
    fn aviso_backends(&self) -> &'static str {
        if self.backends.is_empty() { "; los backends no se comprueban sin daemon" } else { "" }
    }

    /// The real parser's verdict on the YAML in progress.
    ///
    /// Without a daemon the `BACKENDS` event does not arrive, so it is not known which backends exist or which is the
    /// system's. Judging with an empty list would condemn every flow (`validate` rejects any
    /// backend not in it) and the editor would end up read-only exactly when the daemon does not
    /// answer (spec §8). So, without a catalogue, a **copy** of the flow is judged with the backends
    /// it declares itself: the backend is not checked (the daemon will do that when it answers) but everything
    /// else is. Forgiving the backend error is not enough: `parse` aborts at the first one, so
    /// forgiving it would hide the broken CEL or the badly wired port that came after.
    fn juzgar(&self) -> Validez {
        if !self.backends.is_empty() {
            let backends: Vec<&str> = self.backends.iter().map(String::as_str).collect();
            return match parse_with(
                &self.nombre(),
                &self.doc.yaml(),
                &self.senales,
                &backends,
                &self.backend_por_defecto,
            ) {
                Ok(_) => Validez::valido(),
                Err(e) => Validez::invalido(e.to_string(), nodo_de(&e)),
            };
        }
        let mut def = self.doc.def().clone();
        // The backends the flow itself declares, without repeats: those of the nodes and that of `defaults`.
        let mut declarados: Vec<String> = Vec::new();
        for b in def.nodes.values().filter_map(|d| d.backend.clone()).chain(def.defaults.backend.clone()) {
            if !b.is_empty() && !declarados.contains(&b) {
                declarados.push(b);
            }
        }
        // The copy is judged with those backends and with the first as the default, so that a flow that does not
        // declare `defaults.backend` is not condemned for that either. The text in progress is not touched.
        def.defaults.backend = declarados.first().cloned();
        let backends: Vec<&str> = declarados.iter().map(String::as_str).collect();
        let por_defecto = declarados.first().cloned().unwrap_or_default();
        let yaml = serde_yaml::to_string(&def).unwrap_or_default();
        match parse_with(&self.nombre(), &yaml, &self.senales, &backends, &por_defecto) {
            Ok(_) => Validez::valido(),
            Err(e) => Validez::invalido(e.to_string(), nodo_de(&e)),
        }
    }

    /// Edits a field of a node. An empty text deletes the optional field.
    pub fn campo(&mut self, nodo: &str, campo: &Campo, valor: &str) -> Result<(), String> {
        if let Some(razon) = self.doc.solo_lectura() {
            return Err(format!("el YAML no se pudo leer: {razon}"));
        }
        let d = self.doc.def_mut().nodes.get_mut(nodo).ok_or_else(|| format!("nodo desconocido: {nodo}"))?;
        match campo {
            Campo::Instructions => d.instructions = valor.to_string(),
            Campo::When => d.when = opcion(valor),
            Campo::Threshold => d.threshold = numero(valor)?,
            Campo::MinConfidence => d.min_confidence = numero(valor)?,
            Campo::Backend => d.backend = opcion(valor),
            Campo::Model => d.model = opcion(valor),
            Campo::Else => d.else_branch = opcion(valor),
            Campo::RouteWhen(i) => {
                let regla = d.route.get_mut(*i).ok_or_else(|| format!("{nodo} no tiene una regla route {i}"))?;
                regla.when = valor.to_string();
            }
            Campo::RouteTo(i) => {
                let regla = d.route.get_mut(*i).ok_or_else(|| format!("{nodo} no tiene una regla route {i}"))?;
                regla.to = valor.to_string();
            }
            Campo::Opcion(clave) => match &mut d.criteria {
                Some(Criteria::Options(m)) => {
                    let entrada = m.get_mut(clave).ok_or_else(|| format!("{nodo} no declara el criterio {clave}"))?;
                    *entrada = opcion(valor);
                }
                _ => return Err(format!("{nodo} no declara criterios con opciones")),
            },
            Campo::Nivel(i) => match &mut d.criteria {
                Some(Criteria::Levels(niveles)) => {
                    if valor.trim().is_empty() {
                        return Err("un nivel sin nombre no vale".into());
                    }
                    let nivel = niveles.get_mut(*i).ok_or_else(|| format!("{nodo} no tiene un nivel {i}"))?;
                    *nivel = valor.to_string();
                }
                _ => return Err(format!("{nodo} no declara niveles")),
            },
        }
        self.revisar();
        Ok(())
    }

    /// The value a field has today, for the inspector to show and edit.
    pub fn valor(&self, nodo: &str, campo: &Campo) -> Option<String> {
        let d = self.doc.def().nodes.get(nodo)?;
        Some(match campo {
            Campo::Instructions => d.instructions.clone(),
            Campo::When => d.when.clone().unwrap_or_default(),
            Campo::Threshold => numero_texto(d.threshold),
            Campo::MinConfidence => numero_texto(d.min_confidence),
            Campo::Backend => d.backend.clone().unwrap_or_default(),
            Campo::Model => d.model.clone().unwrap_or_default(),
            Campo::Else => d.else_branch.clone().unwrap_or_default(),
            Campo::RouteWhen(i) => d.route.get(*i)?.when.clone(),
            Campo::RouteTo(i) => d.route.get(*i)?.to.clone(),
            Campo::Opcion(clave) => match &d.criteria {
                Some(Criteria::Options(m)) => m.get(clave).cloned().flatten().unwrap_or_default(),
                _ => return None,
            },
            Campo::Nivel(i) => match &d.criteria {
                Some(Criteria::Levels(l)) => l.get(*i)?.clone(),
                _ => return None,
            },
        })
    }

    /// Adds an option to a `choice` (or a `true`/`false` criterion to a `noul`). The port is born **unwired**:
    /// the parser reports it until it is rewired, which is the truth and not a patch-up.
    pub fn anadir_opcion(&mut self, nodo: &str, clave: &str) -> Result<(), String> {
        let clave = clave.trim();
        if clave.is_empty() {
            return Err("la opción necesita nombre".into());
        }
        let d = self.doc.def_mut().nodes.get_mut(nodo).ok_or_else(|| format!("nodo desconocido: {nodo}"))?;
        match &mut d.criteria {
            Some(Criteria::Options(m)) => {
                if m.contains_key(clave) {
                    return Err(format!("la opción {clave} ya existe en {nodo}"));
                }
                m.insert(clave.to_string(), None);
            }
            Some(Criteria::Levels(_)) => return Err(format!("{nodo} es un score: tiene niveles, no opciones")),
            None => return Err(format!("{nodo} no declara criteria; escríbelo a mano en el fichero")),
        }
        self.revisar();
        Ok(())
    }

    /// Removes an option and its port: without the option, the port no longer exists for the parser.
    pub fn quitar_opcion(&mut self, nodo: &str, clave: &str) -> Result<(), String> {
        let d = self.doc.def_mut().nodes.get_mut(nodo).ok_or_else(|| format!("nodo desconocido: {nodo}"))?;
        match &mut d.criteria {
            Some(Criteria::Options(m)) => {
                if m.remove(clave).is_none() {
                    return Err(format!("{nodo} no declara el criterio {clave}"));
                }
                d.out.remove(clave);
            }
            _ => return Err(format!("{nodo} no declara criterios con opciones")),
        }
        self.revisar();
        Ok(())
    }

    /// Adds a node of the requested type with a free name (`nuevo-1`, `nuevo-2`…) and returns it.
    pub fn anadir(&mut self, tipo: NodeType) -> Result<String, String> {
        if let Some(razon) = self.doc.solo_lectura() {
            return Err(format!("el YAML no se pudo leer: {razon}"));
        }
        let def = self.doc.def_mut();
        let nombre = (1..)
            .map(|n| format!("nuevo-{n}"))
            .find(|n| !def.nodes.contains_key(n) && !def.outputs.contains_key(n))
            .ok_or("no hay nombre libre para el nodo nuevo")?;
        def.nodes.insert(nombre.clone(), nodo_nuevo(tipo));
        self.revisar();
        Ok(nombre)
    }

    /// Deletes a node. It refuses if any port, `route` rule, `else` or the `start` references it, and
    /// lists them: deleting blindly would leave the flow with a destination that does not exist. The references are
    /// checked **before** requiring the name to be a node: deleting an output is just as dangerous, and
    /// whoever tries deserves the list of those citing it, not an "unknown" that explains nothing.
    pub fn borrar(&mut self, nodo: &str) -> Result<(), String> {
        let referencias = self.referencias(nodo);
        if !referencias.is_empty() {
            return Err(format!("no se puede borrar {nodo}: lo referencian {}", referencias.join(", ")));
        }
        if self.doc.def_mut().nodes.remove(nodo).is_none() {
            return Err(format!("nodo desconocido: {nodo}"));
        }
        self.revisar();
        Ok(())
    }

    /// Ports, `route` rules, `else` and `start` that point to `nodo`. It does **not** look at the
    /// `$answer/<node>` references of the outputs: they are not a graph destination, and whoever leaves them dangling
    /// finds out from the parser, not from here (see the test that spells it out below).
    pub fn referencias(&self, nodo: &str) -> Vec<String> {
        let def = self.doc.def();
        let mut referencias = Vec::new();
        if def.start == nodo {
            referencias.push("start".to_string());
        }
        for (nombre, d) in &def.nodes {
            for (puerto, destino) in &d.out {
                if destino == nodo {
                    referencias.push(format!("{nombre}.{puerto}"));
                }
            }
            for (i, regla) in d.route.iter().enumerate() {
                if regla.to == nodo {
                    referencias.push(format!("{nombre}.route[{i}]"));
                }
            }
            if d.else_branch.as_deref() == Some(nodo) {
                referencias.push(format!("{nombre}.else"));
            }
        }
        referencias
    }

    /// Wires `puerto` of `nodo` to the requested destination; the destination must exist in the draft.
    pub fn cablear(&mut self, nodo: &str, puerto: &str, destino: &str) -> Result<(), String> {
        if !self.destinos().iter().any(|d| d == destino) {
            return Err(format!("destino desconocido: {destino}"));
        }
        let destino = destino.to_string();
        let puerto = puerto.to_string();
        let d = self.doc.def_mut().nodes.get_mut(nodo).ok_or_else(|| format!("nodo desconocido: {nodo}"))?;
        d.out.insert(puerto, destino);
        self.revisar();
        Ok(())
    }

    /// The possible destinations: the nodes and then the outputs, each group in name order.
    pub fn destinos(&self) -> Vec<String> {
        let def = self.doc.def();
        let mut destinos: Vec<String> = def.nodes.keys().cloned().collect();
        destinos.extend(def.outputs.keys().cloned());
        destinos
    }

    /// Saves the draft by patches. It refuses while there is a validation error (D5).
    pub fn guardar(&mut self) -> Result<usize, String> {
        if let Some(razon) = self.doc.solo_lectura() {
            return Err(format!("el YAML no se pudo leer: {razon}"));
        }
        if !self.validez.ok {
            let motivo = self.validez.error.clone().unwrap_or_default();
            return Err(format!("no se guarda un flujo inválido: {motivo}"));
        }
        self.doc.guardar()
    }

    /// Saves the draft to **another** file (the way out when the original file changed
    /// underneath). It refuses just like `guardar`: an invalid flow does not reach the disk from the editor,
    /// neither the usual file nor a new one (D5).
    pub fn guardar_como(&self, ruta: &Path) -> Result<(), String> {
        if !self.validez.ok {
            let motivo = self.validez.error.clone().unwrap_or_default();
            return Err(format!("no se guarda un flujo inválido: {motivo}"));
        }
        self.doc.guardar_como(ruta)
    }

    /// Reloads from disk, discarding the draft.
    pub fn recargar(&mut self) -> Result<(), String> {
        self.doc.recargar()?;
        self.revisar();
        Ok(())
    }
}

/// The node a `FlowError` points at, if the error names one; whole-flow ones do not.
pub fn nodo_de(e: &FlowError) -> Option<String> {
    let nodo = match e {
        FlowError::UnknownTarget { node, .. }
        | FlowError::MissingPort { node, .. }
        | FlowError::UnknownPort { node, .. }
        | FlowError::BadCriteria { node, .. }
        | FlowError::BadExpression { node, .. }
        | FlowError::BadRanges { node, .. }
        | FlowError::OutOfRange { node, .. }
        | FlowError::BadReference { node, .. }
        | FlowError::UnknownSignal { node, .. }
        | FlowError::UnknownBackend { node, .. } => node,
        FlowError::MissingModel(n) => n,
        FlowError::Cycle(n) | FlowError::Unreachable(n) => n,
        FlowError::BadSignal { .. }
        | FlowError::Yaml(_)
        | FlowError::BadName(_)
        | FlowError::NameClash(_)
        | FlowError::UnknownStart(_) => {
            return None;
        }
    };
    Some(nodo.clone())
}

/// Empty text means "no value": this is how an optional field is deleted.
fn opcion(valor: &str) -> Option<String> {
    (!valor.trim().is_empty()).then(|| valor.to_string())
}

/// An optional number; an error if what was typed is not one.
fn numero(valor: &str) -> Result<Option<f64>, String> {
    let valor = valor.trim();
    if valor.is_empty() {
        return Ok(None);
    }
    valor.parse::<f64>().map(Some).map_err(|e| format!("no es un número: {valor} ({e})"))
}

/// A number as text; empty if there is none.
fn numero_texto(valor: Option<f64>) -> String {
    valor.map_or(String::new(), |v| v.to_string())
}

/// A new node of the requested type, with the minimum for the parser to judge it. It is born **unwired** on
/// purpose: its ports show in the graph and the parser asks for them to be wired. A `batch` is born with one
/// question and one rule, empty, because without them there would be nothing to edit in the inspector.
pub fn nodo_nuevo(tipo: NodeType) -> NodeDef {
    NodeDef {
        ty: tipo,
        instructions: String::new(),
        // A `rule` without `when` is not valid, and the parser says so; it is left empty so it gets written.
        when: (tipo == NodeType::Rule).then(String::new),
        criteria: match tipo {
            NodeType::Choice => Some(Criteria::Options(BTreeMap::from([
                ("opcion-1".to_string(), None),
                ("opcion-2".to_string(), None),
            ]))),
            NodeType::Score => Some(Criteria::Levels(vec!["bajo".to_string(), "alto".to_string()])),
            _ => None,
        },
        threshold: None,
        backend: None,
        model: None,
        min_confidence: None,
        questions: match tipo {
            // A question cannot be a `rule` or a `batch`: it is a `noul`, which is the simplest form.
            NodeType::Batch => BTreeMap::from([("pregunta_1".to_string(), nodo_nuevo(NodeType::Noul))]),
            _ => BTreeMap::new(),
        },
        route: match tipo {
            NodeType::Batch => vec![RouteDef { when: "true".to_string(), to: String::new() }],
            _ => Vec::new(),
        },
        else_branch: None,
        signals: Vec::new(),
        out: BTreeMap::new(),
    }
}

// crates/jevalan-edit/src/edicion.rs — at the end of the file
#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::pruebas::{puerto_muerto, servidor};
    use std::path::PathBuf;

    const LOCAL: &str = include_str!("../../jevalan-flow/fixtures/tool-risk-local-v1.yaml");

    /// A flow's minimal `choice`, for the option tests.
    const CHOICE: &str = r#"
defaults: { backend: kev, model: "kev-latest" }
start: kind
nodes:
  kind:
    type: choice
    instructions: "¿Qué es?"
    criteria: { a: "A", b: "B" }
    out: { a: allow, b: allow }
outputs:
  allow: { outcome: ok }
"#;

    /// Temporary directory that is deleted when dropped.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            // Each directory needs its own name: the tests run in parallel and share
            // `temp_dir()`; with a fixed name, one test's `remove_dir_all` deletes another's.
            static SIGUIENTE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let unico = SIGUIENTE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nombre =
                format!("jevalan-edicion-{}-{}", std::process::id(), crate::documento::huella(&unico.to_string()));
            let dir = std::env::temp_dir().join(nombre);
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn put(&self, nombre: &str, texto: &str) -> PathBuf {
            let ruta = self.0.join(nombre);
            std::fs::write(&ruta, texto).unwrap();
            ruta
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A draft ready to edit, with the catalogue of `config/lab/config.local.yaml`.
    fn borrador(texto: &str, nombre: &str) -> (Tmp, Borrador) {
        let tmp = Tmp::new();
        let ruta = tmp.put(nombre, texto);
        let mut borrador = Borrador::nuevo(Documento::abrir(&ruta).unwrap());
        borrador.catalogo(vec!["kev".into(), "laya".into()], "kev".into());
        borrador.revisar();
        (tmp, borrador)
    }

    #[test]
    fn el_catalogo_de_backends_es_el_que_juzga_los_backends() {
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        assert!(borrador.validez.ok, "{:?}", borrador.validez);
        // Our flow declares `defaults: { backend: kev }`, so with the daemon down and an invented
        // catalogue that does not include it, it is reported; with one that does, it is approved. The backend
        // that rules is the declared one, not the one the editor assumes.
        borrador.catalogo(vec!["laya".into()], "laya".into());
        borrador.revisar();
        assert!(!borrador.validez.ok, "kev no está en ese catálogo: {:?}", borrador.validez);
        assert!(borrador.validez.error.as_deref().unwrap().contains("backend desconocido"), "{:?}", borrador.validez);
    }

    #[test]
    fn sin_daemon_el_editor_sigue_sirviendo_para_guardar() {
        // Without a catalogue (no daemon) the backend cannot be judged, so it is forgiven; but it is said,
        // so that nobody confuses "I could not check it" with "it is fine".
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        borrador.catalogo(Vec::new(), String::new());
        borrador.revisar();
        assert!(borrador.validez.ok, "sin daemon el editor no ha de quedar de solo lectura: {:?}", borrador.validez);
        assert!(
            borrador.aviso_validacion.as_deref().is_some_and(|a| a.contains("backends")),
            "y ha de avisar de que los backends no se comprueban: {:?}",
            borrador.aviso_validacion
        );
        borrador.campo("risks", &Campo::Instructions, "otra cosa").unwrap();
        assert_eq!(borrador.guardar().unwrap(), 1, "y ha de poder guardar");
    }

    #[test]
    fn sin_daemon_lo_demas_se_sigue_juzgando() {
        // Forgiving the backend cannot open the hand to everything else: a broken CEL is still reported.
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        borrador.catalogo(Vec::new(), String::new());
        borrador.campo("risks", &Campo::RouteWhen(1), "answers.risks.destructive >=").unwrap();
        assert!(!borrador.validez.ok, "la CEL rota ha de seguir denunciándose: {:?}", borrador.validez);
        assert_eq!(borrador.validez.nodo.as_deref(), Some("risks.route"));
        assert!(borrador.guardar().is_err(), "y no se ha de poder guardar");
    }

    #[test]
    fn editar_una_regla_route_deja_el_borrador_invalido_y_señala_la_regla() {
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        borrador.campo("risks", &Campo::RouteWhen(1), "answers.risks.destructive >=").unwrap();
        assert!(!borrador.validez.ok);
        assert_eq!(borrador.validez.nodo.as_deref(), Some("risks.route"));
        assert!(borrador.guardar().is_err(), "no se guarda un flujo inválido (D5)");

        // The change from the manual script: 0.55 → 0.6 in the `ask` rule.
        let cuando = "answers.risks.destructive >= 0.6 || answers.risks.exfiltration >= 0.6 || answers.risks.privilege >= 0.6 || answers.risks.secret_exposure >= 0.6";
        borrador.campo("risks", &Campo::RouteWhen(1), cuando).unwrap();
        assert!(borrador.validez.ok, "{:?}", borrador.validez);
        assert_eq!(borrador.guardar().unwrap(), 1, "y entonces sí se guarda, y solo el nodo tocado");
        assert!(std::fs::read_to_string(borrador.doc.ruta()).unwrap().contains(">= 0.6"));
    }

    #[test]
    fn un_umbral_que_no_es_numero_no_se_acepta_y_no_toca_el_borrador() {
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        let antes = borrador.valor("risks", &Campo::Backend).unwrap();
        let error = borrador.campo("risks", &Campo::MinConfidence, "mucha").unwrap_err();
        assert!(error.contains("no es un número"), "{error}");
        assert_eq!(borrador.valor("risks", &Campo::Backend).unwrap(), antes);
        assert!(borrador.validez.ok, "y el flujo sigue válido: {:?}", borrador.validez);
    }

    #[test]
    fn anadir_una_opcion_deja_su_puerto_sin_cablear_y_quitar_se_lo_lleva() {
        // The local binding cannot be called `borrador`: it would shadow the helper function of the same name.
        let (_tmp, mut kind) = borrador(CHOICE, "kind.yaml");
        assert!(kind.validez.ok, "{:?}", kind.validez);

        kind.anadir_opcion("kind", "c").unwrap();
        assert_eq!(kind.validez.error.as_deref(), Some("kind no cablea el puerto \"c\""));

        kind.quitar_opcion("kind", "c").unwrap();
        assert!(kind.validez.ok, "{:?}", kind.validez);

        // A `batch` has no options: it says so instead of inventing them.
        let (_tmp, mut de_batch) = borrador(LOCAL, "tool-risk-local.yaml");
        assert!(de_batch.anadir_opcion("risks", "otra").is_err());
    }

    #[test]
    fn el_nodo_culpable_lo_nombra_el_parser() {
        assert_eq!(nodo_de(&FlowError::MissingPort { node: "kind".into(), port: "c".into() }), Some("kind".into()));
        assert_eq!(nodo_de(&FlowError::Cycle("a".into())), Some("a".into()));
        // Whole-flow errors point at nobody.
        assert_eq!(nodo_de(&FlowError::UnknownStart("nadie".into())), None);
        assert_eq!(nodo_de(&FlowError::Yaml("basura".into())), None);
    }

    /// A flow with a node nobody cites: no port, no `route` rule, no output with
    /// `$answer/...`. It is the only one that can be deleted and saved, and it carries a comment on purpose.
    /// The order of the edits does not matter since `patch::aplicar` merges replacements and deletions
    /// in a single bottom-up pass; before, replacing a node earlier than the deletion misplaced
    /// the second one's indices.
    const SIN_CITAS: &str = r#"# tool.risk servido en local: el comentario que ha de sobrevivir
defaults: { backend: kev, model: "kev-latest" }
start: puerta
nodes:
  sobra:
    type: noul
    instructions: "¿Sobra algo?"
    out: { "true": allow, "false": allow }
  puerta:
    type: rule
    when: 'has(input.command)'
    out: { "true": allow, "false": sobra }
outputs:
  allow: { outcome: ok }
"#;

    #[test]
    fn un_nodo_nuevo_nace_sin_cablear_y_el_parser_lo_dice() {
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        let nombre = borrador.anadir(NodeType::Noul).unwrap();
        assert_eq!(nombre, "nuevo-1");
        assert!(!borrador.validez.ok, "un nodo sin cablear no vale, y eso es lo que se quiere saber");
        assert_eq!(borrador.validez.nodo.as_deref(), Some("nuevo-1"));
        assert_eq!(borrador.validez.error.as_deref(), Some("nuevo-1 no cablea el puerto \"true\""));

        // Its ports are wired and all it lacks is for someone to reach it.
        for puerto in ["true", "false"] {
            borrador.cablear(&nombre, puerto, "allow").unwrap();
        }
        assert_eq!(borrador.validez.error.as_deref(), Some("el nodo nuevo-1 no es alcanzable desde start"));

        // A `batch` is born with one question and one rule, empty, so they can be filled in from the inspector.
        let batch = borrador.anadir(NodeType::Batch).unwrap();
        let d = &borrador.doc.def().nodes[&batch];
        assert_eq!(d.questions.keys().collect::<Vec<_>>(), ["pregunta_1"]);
        assert_eq!(d.route.len(), 1);
        assert_eq!(d.route[0].when, "true");
    }

    #[test]
    fn borrar_un_nodo_referenciado_se_rechaza_y_lista_los_puertos() {
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        let error = borrador.borrar("risks").unwrap_err();
        assert_eq!(error, "no se puede borrar risks: lo referencian pipe_sh.false");

        // `rm_root` is the `start`: it does not simply leave either.
        let error = borrador.borrar("rm_root").unwrap_err();
        assert!(error.contains("start"), "{error}");

        // An output is not a node, and its references are listed just the same.
        let error = borrador.borrar("allow").unwrap_err();
        assert!(error.contains("risks.else"), "{error}");
    }

    #[test]
    fn cablear_exige_un_destino_que_exista_y_deja_el_borrador_valido() {
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        assert_eq!(borrador.destinos().len(), 9, "tres nodos y seis salidas");
        assert!(borrador.cablear("rm_root", "true", "ninguno").is_err());

        borrador.cablear("rm_root", "true", "allow").unwrap();
        assert_eq!(borrador.doc.def().nodes["rm_root"].out["true"], "allow");
        assert!(borrador.validez.ok, "{:?}", borrador.validez);
    }

    #[test]
    fn borrar_un_nodo_sin_referencias_reescribe_solo_lo_tocado() {
        let (_tmp, mut borrador) = borrador(SIN_CITAS, "sin-citas.yaml");
        // The port that referenced it is rewired, and only then can it be deleted.
        borrador.cablear("puerta", "false", "allow").unwrap();
        borrador.borrar("sobra").unwrap();
        assert!(borrador.validez.ok, "{:?}", borrador.validez);
        assert_eq!(borrador.guardar().unwrap(), 2, "el puerto recableado y el nodo borrado");

        let texto = std::fs::read_to_string(borrador.doc.ruta()).unwrap();
        let leido: jevalan_flow::FlowDef = serde_yaml::from_str(&texto).unwrap();
        assert!(!leido.nodes.contains_key("sobra"), "el borrado se llevó su tramo: {texto}");
        assert_eq!(leido.nodes["puerta"].out["false"], "allow", "y el puerto recableado quedó escrito");
        assert!(texto.contains("# tool.risk servido en local"), "y los comentarios siguen: {texto}");
    }

    /// The limit the plan did not foresee: `risks` cannot be deleted from the local flow even by rewiring its
    /// port, because the outputs cite its answers with `$answer/risks` and the parser reports it.
    /// It is written down so the trap (a draft that can no longer be saved) is not discovered
    /// in production. When outputs can be edited or pruned, this test will change.
    #[test]
    fn borrar_un_nodo_que_una_salida_cita_deja_el_borrador_sin_poder_guardarse() {
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        borrador.cablear("pipe_sh", "false", "allow").unwrap();
        borrador.borrar("risks").unwrap();
        assert!(!borrador.validez.ok, "las salidas siguen citando $answer/risks");
        assert_eq!(borrador.validez.nodo.as_deref(), Some("allow"), "la primera salida que lo cita");
        assert!(borrador.validez.error.as_deref().unwrap().contains("$answer/risks"), "{:?}", borrador.validez);
        assert!(borrador.guardar().is_err(), "y sin validez no se guarda (D5)");
    }

    #[test]
    fn con_daemon_manda_el_veredicto_del_daemon_y_el_nodo_lo_señala_el_parser() {
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        assert!(borrador.validez.ok);
        assert!(borrador.aviso_validacion.is_none());

        // A daemon that says no, on a draft the local parser also rejects.
        borrador.campo("risks", &Campo::RouteWhen(1), "answers.risks.destructive >=").unwrap();
        borrador
            .con_daemon(Some(Cliente::nuevo(&servidor(r#"{"ok":false,"error":"lo dice el daemon"}"#), None).unwrap()));
        borrador.revisar();
        assert!(!borrador.validez.ok);
        assert_eq!(borrador.validez.error.as_deref(), Some("lo dice el daemon"), "manda el daemon");
        assert_eq!(borrador.validez.nodo.as_deref(), Some("risks.route"), "y el nodo, el parser local");
    }

    #[test]
    fn sin_daemon_la_validacion_cae_al_parser_local_y_lo_avisa() {
        let (_tmp, mut borrador) = borrador(LOCAL, "tool-risk-local.yaml");
        borrador.con_daemon(Some(Cliente::nuevo(&puerto_muerto(), None).unwrap()));
        borrador.revisar();
        assert!(borrador.validez.ok, "el parser local aprueba el flujo: {:?}", borrador.validez);
        assert!(
            borrador.aviso_validacion.as_deref().unwrap().contains("validación local"),
            "{:?}",
            borrador.aviso_validacion
        );

        // And with the daemon down, a flow error is still shown.
        borrador.campo("risks", &Campo::RouteWhen(1), "answers.risks.destructive >=").unwrap();
        assert!(!borrador.validez.ok);
        assert_eq!(borrador.validez.nodo.as_deref(), Some("risks.route"));
    }

    #[test]
    fn el_borrador_se_juzga_con_el_catalogo_de_senales() {
        let flujo = "start: g
nodes:
  g: { type: rule, when: 'signals.root', out: { \"true\": a, \"false\": b } }
outputs:
  a: { outcome: ok }
  b: { outcome: ok }
";
        let (_tmp, mut borrador) = borrador(flujo, "guarda.yaml");
        assert_eq!(borrador.validez.error.as_deref(), Some("g: señal desconocida \"root\""));
        assert_eq!(borrador.validez.nodo.as_deref(), Some("g"));
        let raiz = jevalan_flow::parse_signal(
            "root",
            "type: rule
when: 'input.x == 1'
",
        )
        .unwrap();
        borrador.senales(jevalan_flow::Signals::from([("root".to_string(), raiz)]));
        borrador.revisar();
        assert!(borrador.validez.ok, "{:?}", borrador.validez.error);
    }
}
