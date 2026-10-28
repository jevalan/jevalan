//! Documento: el texto tal cual, el `FlowDef` en curso y la huella del fichero que se abrió.

use crate::patch::{self, Parche};
use jevalan_flow::FlowDef;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Lo que hay que reescribir al guardar: el diff entre el `FlowDef` que se leyó y el que está en curso.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cambios {
    /// Nodos cuyo cuerpo cambió.
    pub sustituidos: Vec<String>,
    /// Nodos que no estaban.
    pub nuevos: Vec<String>,
    /// Nodos que se borraron.
    pub borrados: Vec<String>,
}

impl Cambios {
    /// Cuántos nodos hay que reescribir en total.
    pub fn total(&self) -> usize {
        self.sustituidos.len() + self.nuevos.len() + self.borrados.len()
    }

    pub fn vacio(&self) -> bool {
        self.total() == 0
    }
}

/// FNV-1a de 64 bits en hexadecimal: identifica el texto leído sin pretender ser criptográfico. Es
/// el mismo cálculo que `jevalan_lab::write::fnv1a64`, copiado para no depender del laboratorio.
pub fn huella(texto: &str) -> String {
    let hash = texto.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{hash:016x}")
}

/// Lo que el editor tiene abierto. Única fuente de verdad de la edición.
pub struct Documento {
    ruta: PathBuf,
    /// El texto que hay en el disco, con sus comentarios y su orden.
    original: String,
    /// El `FlowDef` que se leyó, para saber qué se ha tocado.
    leido: FlowDef,
    /// El `FlowDef` en curso.
    def: FlowDef,
    /// Huella del texto vigente, para detectar que el fichero cambió por debajo.
    huella: String,
    /// Motivo por el que el YAML no se pudo leer; con él, el documento es de solo lectura.
    solo_lectura: Option<String>,
}

impl Documento {
    /// Abre `ruta`. Un YAML que no se entiende entra en **solo lectura**, con su motivo: se puede
    /// ver, pero no editar hasta que el fichero sea válido.
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

    /// El nombre del flujo: el tallo del fichero, que es como lo conoce el daemon.
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

    /// El texto vigente: el que se leyó o el que se escribió por última vez.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// El borrador entero como fichero de flujo, con la misma forma que un `config/flows/*.yaml`. Es lo que
    /// se envía al daemon para que lo juzgue con el parser real.
    pub fn yaml(&self) -> String {
        serde_yaml::to_string(&self.def).unwrap_or_default()
    }

    /// El motivo por el que el documento es de solo lectura, si lo es.
    pub fn solo_lectura(&self) -> Option<&str> {
        self.solo_lectura.as_deref()
    }

    /// El diff entre lo leído y lo que está en curso.
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

    /// ¿El fichero cambió en el disco desde que se abrió, o desde el último guardado?
    pub fn cambio_por_debajo(&self) -> Result<bool, String> {
        let texto = std::fs::read_to_string(&self.ruta)
            .map_err(|e| format!("no se puede leer {}: {e}", self.ruta.display()))?;
        Ok(huella(&texto) != self.huella)
    }

    /// Guarda por parches: sustituye los tramos tocados, borra los eliminados e inserta los nuevos.
    /// Se niega a escribir si algún tramo no se reconoce o si el texto no vuelve a leerse igual.
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

    /// Guarda el borrador en **otro** fichero, sin tocar el original: la salida cuando el fichero
    /// cambió por debajo y no se quiere pisar la mano ajena.
    pub fn guardar_como(&self, ruta: &Path) -> Result<(), String> {
        escribir_atomico(ruta, &self.texto_listo()?).map_err(|e| e.to_string())
    }

    /// Relee el fichero del disco y descarta el borrador.
    pub fn recargar(&mut self) -> Result<(), String> {
        *self = Self::abrir(self.ruta.clone())?;
        Ok(())
    }

    /// El texto del borrador, ya parcheado y comprobado; error si así no se puede escribir.
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
        // Última red: lo que se va a escribir tiene que volver a leerse como el mismo `FlowDef`. Si
        // el emisor de YAML hiciera algo raro con las comillas, no llega al disco.
        match serde_yaml::from_str::<FlowDef>(&texto) {
            Ok(leido) if leido == self.def => Ok(texto),
            Ok(_) => Err("el texto parcheado no vuelve a leerse igual: no se escribe".into()),
            Err(e) => Err(format!("el texto parcheado no es YAML válido: {e}")),
        }
    }
}

/// Un `FlowDef` sin nada: el documento de un YAML que no se pudo leer.
fn vacio() -> FlowDef {
    FlowDef {
        description: String::new(),
        defaults: Default::default(),
        start: String::new(),
        nodes: BTreeMap::new(),
        outputs: BTreeMap::new(),
    }
}

/// Escritura atómica: temporal en el mismo directorio, `sync_all` y renombrado sobre el destino.
/// Deja `<fichero>.bak` con la versión anterior si no existía ya. Sin dependencias nuevas.
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

    /// Directorio temporal que se borra al soltarse.
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
