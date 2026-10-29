//! Document: the text as is, the `FlowDef` in progress and the fingerprint of the file that was opened.

use crate::patch::{self, Parche};
use jevalan_flow::FlowDef;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// What has to be rewritten on save: the diff between the `FlowDef` that was read and the one in progress.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cambios {
    /// Nodes whose body changed.
    pub sustituidos: Vec<String>,
    /// Nodes that were not there.
    pub nuevos: Vec<String>,
    /// Nodes that were deleted.
    pub borrados: Vec<String>,
}

impl Cambios {
    /// How many nodes have to be rewritten in total.
    pub fn total(&self) -> usize {
        self.sustituidos.len() + self.nuevos.len() + self.borrados.len()
    }

    pub fn vacio(&self) -> bool {
        self.total() == 0
    }
}

/// 64-bit FNV-1a in hexadecimal: identifies the text that was read without claiming to be cryptographic. It is
/// the same computation as `jevalan_lab::write::fnv1a64`, copied so as not to depend on the laboratory.
pub fn huella(texto: &str) -> String {
    let hash = texto.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{hash:016x}")
}

/// What the editor has open. The single source of truth for editing.
pub struct Documento {
    ruta: PathBuf,
    /// The text on disk, with its comments and its order.
    original: String,
    /// The `FlowDef` that was read, to know what has been touched.
    leido: FlowDef,
    /// The `FlowDef` in progress.
    def: FlowDef,
    /// Fingerprint of the current text, to detect that the file changed underneath.
    huella: String,
    /// Reason the YAML could not be read; with it, the document is read-only.
    solo_lectura: Option<String>,
}

impl Documento {
    /// Opens `ruta`. A YAML that is not understood enters **read-only**, with its reason: it can be
    /// seen, but not edited until the file is valid.
    pub fn abrir(ruta: impl Into<PathBuf>) -> Result<Self, String> {
        let ruta = ruta.into();
        let original =
            std::fs::read_to_string(&ruta).map_err(|e| format!("no se puede leer {}: {e}", ruta.display()))?;
        let (def, solo_lectura) = match serde_yaml::from_str::<FlowDef>(&original) {
            Ok(def) => (def, None),
            Err(e) => (vacio(), Some(e.to_string())),
        };
        Ok(Self { huella: huella(&original), leido: def.clone(), def, original, ruta, solo_lectura })
    }

    /// The flow's name: the file stem, which is how the daemon knows it.
    pub fn nombre(&self) -> String {
        self.ruta.file_stem().and_then(|n| n.to_str()).unwrap_or_default().to_string()
    }

    pub fn ruta(&self) -> &Path {
        &self.ruta
    }

    pub fn def(&self) -> &FlowDef {
        &self.def
    }

    pub fn def_mut(&mut self) -> &mut FlowDef {
        &mut self.def
    }

    /// The current text: the one that was read or the one last written.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// The whole draft as a flow file, with the same shape as a `config/flows/*.yaml`. It is what
    /// is sent to the daemon so that it judges it with the real parser.
    pub fn yaml(&self) -> String {
        serde_yaml::to_string(&self.def).unwrap_or_default()
    }

    /// The reason the document is read-only, if it is.
    pub fn solo_lectura(&self) -> Option<&str> {
        self.solo_lectura.as_deref()
    }

    /// The diff between what was read and what is in progress.
    pub fn cambios(&self) -> Cambios {
        let mut cambios = Cambios::default();
        for (nombre, def) in &self.def.nodes {
            match self.leido.nodes.get(nombre) {
                Some(antes) if antes == def => {}
                Some(_) => cambios.sustituidos.push(nombre.clone()),
                None => cambios.nuevos.push(nombre.clone()),
            }
        }
        for nombre in self.leido.nodes.keys() {
            if !self.def.nodes.contains_key(nombre) {
                cambios.borrados.push(nombre.clone());
            }
        }
        cambios
    }

    /// Did the file change on disk since it was opened, or since the last save?
    pub fn cambio_por_debajo(&self) -> Result<bool, String> {
        let texto = std::fs::read_to_string(&self.ruta)
            .map_err(|e| format!("no se puede leer {}: {e}", self.ruta.display()))?;
        Ok(huella(&texto) != self.huella)
    }

    /// Saves by patches: replaces the touched spans, deletes the removed ones and inserts the new ones.
    /// Refuses to write if any span is not recognised or if the text does not read back the same.
    pub fn guardar(&mut self) -> Result<usize, String> {
        if let Some(razon) = &self.solo_lectura {
            return Err(format!("el YAML no se pudo leer: {razon}"));
        }
        let texto = self.texto_listo()?;
        escribir_atomico(&self.ruta, &texto)?;
        let cuantos = self.cambios().total();
        self.original = texto;
        self.leido = self.def.clone();
        self.huella = huella(&self.original);
        Ok(cuantos)
    }

    /// Saves the draft to **another** file, without touching the original: the way out when the file
    /// changed underneath and someone else's hand is not to be trampled.
    pub fn guardar_como(&self, ruta: &Path) -> Result<(), String> {
        escribir_atomico(ruta, &self.texto_listo()?).map_err(|e| e.to_string())
    }

    /// Re-reads the file from disk and discards the draft.
    pub fn recargar(&mut self) -> Result<(), String> {
        *self = Self::abrir(self.ruta.clone())?;
        Ok(())
    }

    /// The draft text, already patched and checked; an error if it cannot be written that way.
    fn texto_listo(&self) -> Result<String, String> {
        let cambios = self.cambios();
        let mut parches = Vec::new();
        for nombre in &cambios.borrados {
            parches.push(Parche::Borrar { nombre: nombre.clone() });
        }
        for nombre in &cambios.sustituidos {
            parches.push(Parche::Sustituir {
                nombre: nombre.clone(),
                lineas: patch::bloque(nombre, &self.def.nodes[nombre]),
            });
        }
        for nombre in &cambios.nuevos {
            parches.push(Parche::Insertar {
                nombre: nombre.clone(),
                lineas: patch::bloque(nombre, &self.def.nodes[nombre]),
            });
        }
        let (texto, perdidos) = patch::aplicar(&self.original, &parches);
        if !perdidos.is_empty() {
            return Err(format!("sin tramo que parchear: {}", perdidos.join(", ")));
        }
        // Last safety net: what is about to be written has to read back as the same `FlowDef`. If
        // the YAML emitter did something odd with the quotes, it does not reach the disk.
        match serde_yaml::from_str::<FlowDef>(&texto) {
            Ok(leido) if leido == self.def => Ok(texto),
            Ok(_) => Err("el texto parcheado no vuelve a leerse igual: no se escribe".into()),
            Err(e) => Err(format!("el texto parcheado no es YAML válido: {e}")),
        }
    }
}

/// An empty `FlowDef`: the document of a YAML that could not be read.
fn vacio() -> FlowDef {
    FlowDef {
        description: String::new(),
        defaults: Default::default(),
        start: String::new(),
        nodes: BTreeMap::new(),
        outputs: BTreeMap::new(),
    }
}

/// Atomic write: temporary file in the same directory, `sync_all` and rename over the destination.
/// Leaves `<file>.bak` with the previous version if one did not already exist. No new dependencies.
fn escribir_atomico(ruta: &Path, texto: &str) -> Result<(), String> {
    let carpeta = ruta.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let nombre = ruta.file_name().and_then(|n| n.to_str()).ok_or("la ruta no nombra un fichero")?;
    let temporal = carpeta.join(format!(".{nombre}.tmp"));
    let copia = carpeta.join(format!("{nombre}.bak"));
    {
        let mut fichero =
            std::fs::File::create(&temporal).map_err(|e| format!("no se puede crear {}: {e}", temporal.display()))?;
        fichero.write_all(texto.as_bytes()).map_err(|e| e.to_string())?;
        fichero.sync_all().map_err(|e| e.to_string())?;
    }
    if ruta.exists() && !copia.exists() {
        std::fs::copy(ruta, &copia).map_err(|e| format!("no se puede dejar {}: {e}", copia.display()))?;
    }
    std::fs::rename(&temporal, ruta).map_err(|e| {
        let _ = std::fs::remove_file(&temporal);
        format!("no se puede escribir {}: {e}", ruta.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Temporary directory that is deleted when dropped.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            let nombre =
                format!("jevalan-edit-{}-{}", std::process::id(), huella(&format!("{:?}", std::time::Instant::now())));
            let dir = std::env::temp_dir().join(nombre);
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

    const LOCAL: &str = include_str!("../../jevalan-flow/fixtures/tool-risk-local-v1.yaml");

    #[test]
    fn guardar_reescribe_solo_lo_tocado_y_deja_bak() {
        let tmp = Tmp::new();
        let ruta = tmp.put("tool-risk-local.yaml", LOCAL);
        let mut doc = Documento::abrir(&ruta).unwrap();
        assert!(doc.solo_lectura().is_none());
        assert!(doc.cambios().vacio(), "recién abierto no hay nada que guardar");
        assert_eq!(doc.nombre(), "tool-risk-local");

        let when = doc.def().nodes["risks"].route[1].when.replace("0.55", "0.6");
        doc.def_mut().nodes.get_mut("risks").unwrap().route[1].when = when;
        assert_eq!(doc.cambios().total(), 1);
        assert_eq!(doc.cambios().sustituidos, ["risks"]);

        assert_eq!(doc.guardar().unwrap(), 1);
        let escrito = std::fs::read_to_string(&ruta).unwrap();
        assert!(escrito.contains(">= 0.6") && !escrito.contains(">= 0.55"), "{escrito}");
        assert!(escrito.contains("# tool.risk servido en local"), "los comentarios sobreviven: {escrito}");
        assert_eq!(
            std::fs::read_to_string(tmp.0.join("tool-risk-local.yaml.bak")).unwrap(),
            LOCAL,
            "el `.bak` es la versión anterior, y solo se deja una vez"
        );
        assert!(doc.cambios().vacio(), "tras guardar no queda nada pendiente");
        assert!(!doc.cambio_por_debajo().unwrap(), "y el fichero es el que acabamos de escribir");
    }

    #[test]
    fn detecta_que_el_fichero_cambio_por_debajo() {
        let tmp = Tmp::new();
        let ruta = tmp.put("flujo.yaml", LOCAL);
        let mut doc = Documento::abrir(&ruta).unwrap();
        assert!(!doc.cambio_por_debajo().unwrap());

        std::fs::write(&ruta, LOCAL.replace("source: model", "source: model-local")).unwrap();
        assert!(doc.cambio_por_debajo().unwrap(), "y ahora sí");

        doc.recargar().unwrap();
        assert!(!doc.cambio_por_debajo().unwrap());
        assert!(doc.original().contains("model-local"), "recargar trae el texto ajeno");
        assert!(doc.cambios().vacio(), "y descarta el borrador");
    }

    #[test]
    fn un_yaml_imposible_se_abre_en_solo_lectura_y_no_se_guarda() {
        let tmp = Tmp::new();
        for (nombre, texto) in [("roto.yaml", "start: [esto no es un flujo"), ("vacio.yaml", "")] {
            let ruta = tmp.put(nombre, texto);
            let mut doc = Documento::abrir(&ruta).unwrap();
            assert!(doc.solo_lectura().is_some(), "{nombre} debía ser de solo lectura");
            assert!(doc.cambios().vacio(), "{nombre}: no hay nada que parchear");
            assert!(doc.guardar().is_err(), "{nombre}: un documento ilegible no se guarda");
            assert_eq!(std::fs::read_to_string(&ruta).unwrap(), texto, "{nombre}: el fichero no se toca");
        }
    }

    #[test]
    fn guardar_como_deja_el_original_intacto() {
        let tmp = Tmp::new();
        let ruta = tmp.put("tool-risk-local.yaml", LOCAL);
        let mut doc = Documento::abrir(&ruta).unwrap();
        let when = doc.def().nodes["risks"].route[1].when.replace("0.55", "0.6");
        doc.def_mut().nodes.get_mut("risks").unwrap().route[1].when = when;

        let aparte = tmp.0.join("tool-risk-local.editado.yaml");
        doc.guardar_como(&aparte).unwrap();
        assert_eq!(std::fs::read_to_string(&ruta).unwrap(), LOCAL, "el original, byte a byte");
        assert!(std::fs::read_to_string(&aparte).unwrap().contains(">= 0.6"));
        assert!(!tmp.0.join("tool-risk-local.yaml.bak").exists(), "guardar aparte no deja `.bak` del original");
    }

    #[test]
    fn guardar_conserva_los_finales_de_linea() {
        let tmp = Tmp::new();
        let crlf = LOCAL.replace('\n', "\r\n");
        let ruta = tmp.put("tool-risk-local.yaml", &crlf);
        let mut doc = Documento::abrir(&ruta).unwrap();
        let when = doc.def().nodes["risks"].route[1].when.replace("0.55", "0.6");
        doc.def_mut().nodes.get_mut("risks").unwrap().route[1].when = when;
        doc.guardar().unwrap();

        let texto = std::fs::read_to_string(&ruta).unwrap();
        assert!(texto.split('\n').filter(|l| !l.is_empty()).all(|l| l.ends_with('\r')), "{texto}");
        assert_eq!(std::fs::read(tmp.0.join("tool-risk-local.yaml.bak")).unwrap(), crlf.as_bytes());
    }
}
