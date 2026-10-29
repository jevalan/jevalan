//! Editor state: the focus, the open document, its graph and what is being confirmed.

use crate::edicion::{Borrador, TIPOS};
use crate::grafo::Grafo;
use crate::inspector::{Fila, Inspector};
use crate::telemetria::Telemetria;
use crate::vista;
use jevalan_flow::NodeDef;
use jevalan_tui::widgets::flow_graph::FlowGraph;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use std::path::PathBuf;
use std::time::Instant;

/// The three zones of the screen, in the order `tab` walks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foco {
    Grafo,
    Inspector,
    Banda,
}

/// What the editor is asking to confirm.
#[derive(Debug, Clone, PartialEq)]
pub enum Aviso {
    /// Quit with unsaved changes.
    Salir,
    /// Reload from disk, discarding the draft.
    Recargar,
    /// Borrar el nodo nombrado.
    Borrar(String),
    /// The file changed underneath: reload with `r` or save aside with `g`.
    Ajena { ruta: PathBuf },
}

/// What is done with the option chosen in the one-line menu.
#[derive(Debug, Clone, PartialEq)]
pub enum Fin {
    /// Add a node of the chosen type, in the order of `TIPOS`.
    AnadirTipo,
    /// Rewire this port to the chosen destination.
    Cablear { nodo: String, puerto: String },
}

/// One-line menu: what is chosen with the arrows and confirmed with `enter`.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub titulo: String,
    pub opciones: Vec<String>,
    pub cursor: usize,
    pub fin: Fin,
}

impl Menu {
    pub fn nuevo(titulo: String, opciones: Vec<String>, fin: Fin) -> Self {
        Self { titulo, opciones, cursor: 0, fin }
    }
}

/// The editor's whole state.
pub struct App {
    pub borrador: Borrador,
    pub grafo: Grafo,
    pub inspector: Inspector,
    pub telemetria: Telemetria,
    /// The graph painter, reused as is from the dashboard.
    pub pintor: FlowGraph,
    pub foco: Foco,
    /// The open one-line menu (`a` types, `c` destinations).
    pub menu: Option<Menu>,
    pub aviso: Option<Aviso>,
    /// Ephemeral notice in the bottom band.
    pub aviso_banda: Option<String>,
    pub salir: bool,
    pub redibujar: bool,
    pub ahora: Instant,
}

impl App {
    /// Builds the editor with the document already open and its telemetry.
    pub fn nuevo(mut borrador: Borrador, telemetria: Telemetria) -> Self {
        borrador.revisar();
        let mut grafo = Grafo::nuevo();
        grafo.rehacer(&borrador.nombre(), borrador.doc.def());
        Self {
            borrador,
            grafo,
            inspector: Inspector::nuevo(),
            telemetria,
            pintor: FlowGraph::new(),
            foco: Foco::Grafo,
            menu: None,
            aviso: None,
            aviso_banda: None,
            salir: false,
            redibujar: true,
            ahora: Instant::now(),
        }
    }

    /// The name and definition of the selected node, if the stop is a node.
    pub fn nodo(&self) -> Option<(&str, &NodeDef)> {
        let nombre = self.grafo.nodo()?;
        self.borrador.doc.def().nodes.get(nombre).map(|d| (nombre, d))
    }

    /// The inspector rows for the selected node.
    pub fn filas(&self) -> Vec<Fila> {
        self.nodo().map_or_else(Vec::new, |(_, d)| crate::inspector::filas(d))
    }

    /// Rebuilds the graph from the draft and adjusts the inspector cursor.
    pub fn rehacer(&mut self) {
        self.grafo.rehacer(&self.borrador.nombre(), self.borrador.doc.def());
        self.inspector.ajustar(self.filas().len());
        self.redibujar = true;
    }

    /// A key. The house keys work from any zone, as in the TUI.
    pub fn tecla(&mut self, k: KeyEvent) {
        if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        self.redibujar = true;
        // Order matters: first what is being typed, then the menu, then what is being confirmed.
        if self.inspector.texto().is_some() {
            return self.tecla_edicion(k.code);
        }
        if self.menu.is_some() {
            return self.tecla_menu(k.code);
        }
        if self.aviso.is_some() {
            return self.confirmar(k.code);
        }
        match k.code {
            KeyCode::Char('q') => return self.pedir_salir(),
            KeyCode::Tab => return self.cambiar_foco(),
            KeyCode::Char('s') => return self.pedir_guardar(),
            KeyCode::Char('r') => return self.solo_pedir(Aviso::Recargar),
            KeyCode::Char('a') => return self.pedir_anadir(),
            KeyCode::Char('d') => return self.pedir_borrar(),
            KeyCode::Char('c') => return self.pedir_cablear(),
            _ => {}
        }
        match self.foco {
            Foco::Grafo => self.tecla_grafo(k.code),
            Foco::Inspector => self.tecla_inspector(k.code),
            Foco::Banda => {}
        }
    }

    /// Keys while a field is being typed: `enter` confirms it in the draft, `esc` cancels it.
    fn tecla_edicion(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => self.confirmar_campo(),
            KeyCode::Esc => self.inspector.cancelar(),
            KeyCode::Backspace => self.inspector.borrar(),
            KeyCode::Char(c) => self.inspector.escribir(c),
            _ => {}
        }
    }

    /// Confirms the text in progress in the draft, never on disk.
    fn confirmar_campo(&mut self) {
        let Some(texto) = self.inspector.texto().map(str::to_string) else { return };
        let Some((nodo, _)) = self.nodo() else { return };
        let nodo = nodo.to_string();
        let Some(fila) = self.filas().get(self.inspector.cursor).cloned() else { return };
        match self.borrador.campo(&nodo, &fila.campo, &texto) {
            Ok(()) => {
                self.inspector.cancelar();
                self.aviso_banda = None;
                self.rehacer();
            }
            // A value that is not valid leaves the edit open so it can be corrected.
            Err(e) => self.aviso_banda = Some(e),
        }
    }

    /// In the graph, the arrows move the selection across nodes and edges.
    fn tecla_grafo(&mut self, code: KeyCode) {
        match code {
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j') | KeyCode::Char('l') => self.grafo.siguiente(),
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => self.grafo.anterior(),
            _ => {}
        }
        self.inspector.ajustar(self.filas().len());
    }

    /// In the inspector, the arrows walk its rows and `e` enters the selected field.
    fn tecla_inspector(&mut self, code: KeyCode) {
        let filas = self.filas().len();
        match code {
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j') | KeyCode::Char('l') => self.inspector.bajar(filas),
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => self.inspector.subir(),
            KeyCode::Char('e') | KeyCode::Enter => match self.filas().get(self.inspector.cursor).cloned() {
                Some(fila) => self.inspector.empezar(&fila.valor),
                None => self.aviso_banda = Some("no hay ningún campo que editar".into()),
            },
            _ => {}
        }
    }

    /// `s`: saves if the file did not change underneath; if it did, offers to reload or save aside.
    fn pedir_guardar(&mut self) {
        match self.borrador.doc.cambio_por_debajo() {
            Ok(false) => match self.borrador.guardar() {
                Ok(cuantos) => {
                    self.aviso_banda = Some(format!("guardado: {cuantos} nodos reescritos"));
                    self.rehacer();
                }
                Err(e) => self.aviso_banda = Some(e),
            },
            Ok(true) => self.aviso = Some(Aviso::Ajena { ruta: self.borrador.doc.ruta().to_path_buf() }),
            Err(e) => self.aviso_banda = Some(e),
        }
    }

    /// Asks for a simple confirmation.
    fn solo_pedir(&mut self, aviso: Aviso) {
        self.aviso = Some(aviso);
    }

    /// `a`: adds a node; opens the type menu.
    fn pedir_anadir(&mut self) {
        let opciones: Vec<String> = TIPOS.iter().map(|t| t.as_str().to_string()).collect();
        self.menu = Some(Menu::nuevo("añadir nodo".into(), opciones, Fin::AnadirTipo));
    }

    /// `d`: deletes the selected node, with confirmation.
    fn pedir_borrar(&mut self) {
        match self.grafo.nodo() {
            Some(nodo) => self.aviso = Some(Aviso::Borrar(nodo.to_string())),
            None => self.aviso_banda = Some("selecciona un nodo para borrarlo".into()),
        }
    }

    /// `c`: rewires the selected port; opens the destination menu.
    fn pedir_cablear(&mut self) {
        let Some((nodo, puerto)) = self.grafo.puerto().map(|(n, p)| (n.to_string(), p.to_string())) else {
            self.aviso_banda = Some("selecciona un puerto del grafo para recablearlo".into());
            return;
        };
        let destinos = self.borrador.destinos();
        if destinos.is_empty() {
            self.aviso_banda = Some("el borrador no tiene destinos".into());
            return;
        }
        self.menu = Some(Menu::nuevo(format!("recablear {nodo}.{puerto}"), destinos, Fin::Cablear { nodo, puerto }));
    }

    /// The keys of the one-line menu. `esc` closes it without choosing anything: a menu that could not be
    /// dropped would leave the user locked inside it.
    fn tecla_menu(&mut self, code: KeyCode) {
        let Some(mut menu) = self.menu.take() else { return };
        match code {
            KeyCode::Esc => return,
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j') | KeyCode::Char('l') => {
                menu.cursor = (menu.cursor + 1).min(menu.opciones.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => {
                menu.cursor = menu.cursor.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(elegida) = menu.opciones.get(menu.cursor).cloned() {
                    let fin = menu.fin.clone();
                    self.elegir(&fin, &elegida);
                }
                return;
            }
            _ => {}
        }
        self.menu = Some(menu);
    }

    /// Applies the option chosen in the menu.
    fn elegir(&mut self, fin: &Fin, elegida: &str) {
        match fin {
            Fin::AnadirTipo => {
                let Some(tipo) = TIPOS.iter().find(|t| t.as_str() == elegida).copied() else {
                    self.aviso_banda = Some(format!("tipo desconocido: {elegida}"));
                    return;
                };
                match self.borrador.anadir(tipo) {
                    Ok(nombre) => {
                        self.aviso_banda = Some(format!("añadido {nombre}: cablea sus puertos con `c`"));
                        self.rehacer();
                    }
                    Err(e) => self.aviso_banda = Some(e),
                }
            }
            Fin::Cablear { nodo, puerto } => match self.borrador.cablear(nodo, puerto, elegida) {
                Ok(()) => {
                    self.aviso_banda = Some(format!("{nodo}.{puerto} → {elegida}"));
                    self.rehacer();
                }
                Err(e) => self.aviso_banda = Some(e),
            },
        }
    }

    /// `tab` walks the three zones: graph, inspector and band.
    fn cambiar_foco(&mut self) {
        self.foco = match self.foco {
            Foco::Grafo => Foco::Inspector,
            Foco::Inspector => Foco::Banda,
            Foco::Banda => Foco::Grafo,
        };
    }

    /// `q`: quits, warning if there are unsaved changes (D5).
    fn pedir_salir(&mut self) {
        let hay_cambios = !self.borrador.doc.cambios().vacio() && self.borrador.doc.solo_lectura().is_none();
        if hay_cambios {
            self.aviso = Some(Aviso::Salir);
        } else {
            self.salir = true;
        }
    }

    /// A key with a confirmation pending. `esc` always cancels; `enter` or `s` confirm those
    /// that are confirmed that way; the change underneath is handled with `r` or `g`.
    fn confirmar(&mut self, code: KeyCode) {
        let Some(aviso) = self.aviso.take() else { return };
        match (aviso, code) {
            // `esc` always cancels: the notice is dropped and nothing is done. Without this branch, the wildcard
            // below would arm it again and the confirmation could not be abandoned.
            (_, KeyCode::Esc) => {}
            (Aviso::Salir, KeyCode::Enter) => self.salir = true,
            (Aviso::Recargar, KeyCode::Enter) => match self.borrador.recargar() {
                Ok(()) => {
                    self.aviso_banda = Some("recargado del disco; el borrador se descartó".into());
                    self.rehacer();
                }
                Err(e) => self.aviso_banda = Some(e),
            },
            (Aviso::Borrar(nodo), KeyCode::Enter) => match self.borrador.borrar(&nodo) {
                Ok(()) => {
                    self.aviso_banda = Some(format!("borrado {nodo}"));
                    self.rehacer();
                }
                Err(e) => self.aviso_banda = Some(e),
            },
            // The file changed underneath: `r` reloads, `g` saves aside, and nothing is written on its own.
            (Aviso::Ajena { .. }, KeyCode::Char('r')) => match self.borrador.recargar() {
                Ok(()) => {
                    self.aviso_banda = Some("recargado del disco; el borrador se descartó".into());
                    self.rehacer();
                }
                Err(e) => self.aviso_banda = Some(e),
            },
            (Aviso::Ajena { ruta }, KeyCode::Char('g')) => {
                let aparte = editado(&ruta);
                // D5's veto lives in the draft, not here: an invalid flow is neither written aside
                // nor written on its own. If it is refused, the notice stays up so `r` can be pressed.
                match self.borrador.guardar_como(&aparte) {
                    Ok(()) => self.aviso_banda = Some(format!("guardado en {}", aparte.display())),
                    Err(e) => {
                        self.aviso_banda = Some(e);
                        self.aviso = Some(Aviso::Ajena { ruta });
                    }
                }
            }
            (aviso, _) => self.aviso = Some(aviso),
        }
    }

    /// Collects whatever arrives from the bus and, if the backend catalogue changed, revalidates.
    pub fn latir(&mut self) {
        self.ahora = Instant::now();
        let antes = self.telemetria.backends.clone();
        if self.telemetria.latir(self.ahora) {
            self.redibujar = true;
            if self.telemetria.backends != antes {
                // The daemon's default backend does not travel in the event, so none is faked:
                // without it local validation does not check the backends, and says so (spec §8).
                self.borrador.catalogo(self.telemetria.backends.clone(), String::new());
                self.borrador.revisar();
            }
        }
    }

    /// Pinta la pantalla entera.
    pub fn draw(&mut self, f: &mut Frame) {
        self.redibujar = false;
        vista::dibujar(f, self);
    }
}

/// The separate file offered when the original changed underneath: `<name>.editado.yaml`.
fn editado(ruta: &std::path::Path) -> PathBuf {
    let tallo = ruta.file_stem().and_then(|n| n.to_str()).unwrap_or("flujo");
    ruta.with_file_name(format!("{tallo}.editado.yaml"))
}

// crates/jevalan-edit/src/app.rs — at the end of the file (the screen tests go here: they use the whole editor)
#[cfg(test)]
mod tests {
    use super::*;
    use crate::documento::Documento;
    use crate::grafo::Parada;
    use crate::inspector::Campo;
    use jevalan_flow::NodeType;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use std::path::PathBuf;

    const LOCAL: &str = include_str!("../../jevalan-flow/fixtures/tool-risk-local-v1.yaml");

    /// Temporary directory that is deleted when dropped.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            // Each directory needs its own name: the tests run in parallel and share
            // `temp_dir()`; with a fixed name, one test's `remove_dir_all` deletes another's.
            static SIGUIENTE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let unico = SIGUIENTE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nombre = format!("jevalan-app-{}-{}", std::process::id(), crate::documento::huella(&unico.to_string()));
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

    /// An editor with the local flow open and no bus.
    fn editor(texto: &str) -> (Tmp, App) {
        let tmp = Tmp::new();
        let ruta = tmp.put("tool-risk-local.yaml", texto);
        let mut borrador = Borrador::nuevo(Documento::abrir(&ruta).unwrap());
        borrador.catalogo(vec!["kev".into()], "kev".into());
        (tmp, App::nuevo(borrador, Telemetria::apagada("tool-risk-local")))
    }

    fn pinta(app: &mut App, ancho: u16, alto: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(ancho, alto)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        terminal.backend().to_string()
    }

    fn tecla(app: &mut App, code: KeyCode) {
        app.tecla(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn la_cabecera_y_la_banda_cuentan_el_borrador() {
        let (_tmp, mut app) = editor(LOCAL);
        let texto = pinta(&mut app, 100, 30);
        assert!(texto.contains("flujo: tool-risk-local"), "{texto}");
        assert!(texto.contains("sin cambios"), "{texto}");
        assert!(texto.contains("válido · 3 nodos · borrador sin publicar · sin telemetría"), "{texto}");
    }

    #[test]
    fn el_cursor_del_grafo_recorre_nodos_y_puertos() {
        let (_tmp, mut app) = editor(LOCAL);
        assert_eq!(app.grafo.parada(), Some(&Parada::Nodo("rm_root".into())));
        tecla(&mut app, KeyCode::Right);
        assert_eq!(app.grafo.puerto(), Some(("rm_root", "true")));
        // The inspector keeps looking at the same node, because the stop is one of its ports.
        assert_eq!(app.nodo().map(|(n, _)| n), Some("rm_root"));
        let texto = pinta(&mut app, 100, 30);
        assert!(texto.contains("puerto rm_root.true"), "{texto}");
        assert!(texto.contains("nodo: rm_root (rule)"), "{texto}");
    }

    #[test]
    fn el_inspector_muestra_las_preguntas_y_las_reglas_de_un_batch() {
        let (_tmp, mut app) = editor(LOCAL);
        for _ in 0..64 {
            if app.grafo.parada() == Some(&Parada::Nodo("risks".into())) {
                break;
            }
            tecla(&mut app, KeyCode::Right);
        }
        let texto = pinta(&mut app, 110, 30);
        assert!(texto.contains("nodo: risks (batch)"), "{texto}");
        assert!(texto.contains("preguntas destructive · exfiltration · privilege · secret_exposure"), "{texto}");
        assert!(texto.contains("route 2 reglas · else: allow · error: ask_error"), "{texto}");
    }

    #[test]
    fn un_terminal_pequeño_avisa_y_no_pinta_nada_mas() {
        let (_tmp, mut app) = editor(LOCAL);
        let texto = pinta(&mut app, 40, 10);
        assert!(texto.contains("terminal demasiado pequeño"), "{texto}");
        assert!(!texto.contains("tool-risk-local"), "{texto}");
    }

    #[test]
    fn un_yaml_ilegible_se_abre_en_solo_lectura_y_sin_grafo_fantasma() {
        let (_tmp, mut app) = editor("start: [esto no es un flujo");
        let texto = pinta(&mut app, 100, 30);
        assert!(texto.contains("solo lectura: el YAML no parsea"), "{texto}");
        assert!(texto.contains("sin grafo que pintar"), "{texto}");
    }

    #[test]
    fn la_tab_recorre_las_tres_zonas() {
        let (_tmp, mut app) = editor(LOCAL);
        assert_eq!(app.foco, Foco::Grafo);
        tecla(&mut app, KeyCode::Tab);
        assert_eq!(app.foco, Foco::Inspector);
        tecla(&mut app, KeyCode::Tab);
        assert_eq!(app.foco, Foco::Banda);
        tecla(&mut app, KeyCode::Tab);
        assert_eq!(app.foco, Foco::Grafo);
    }

    #[test]
    fn la_q_sale_cuando_no_hay_nada_sin_guardar() {
        let (_tmp, mut app) = editor(LOCAL);
        tecla(&mut app, KeyCode::Char('q'));
        assert!(app.salir && app.aviso.is_none());
    }

    #[test]
    fn la_e_edita_el_campo_y_el_enter_lo_confirma_en_el_borrador() {
        let (_tmp, mut app) = editor(LOCAL);
        tecla(&mut app, KeyCode::Tab);
        tecla(&mut app, KeyCode::Down);
        assert_eq!(app.filas()[app.inspector.cursor].campo, Campo::When);
        tecla(&mut app, KeyCode::Char('e'));
        assert!(app.inspector.texto().is_some(), "la edición está abierta");

        // The editor writes at the end: the expression is deleted and another one written.
        for _ in 0..200 {
            tecla(&mut app, KeyCode::Backspace);
        }
        for c in "has(input.x)".chars() {
            tecla(&mut app, KeyCode::Char(c));
        }
        tecla(&mut app, KeyCode::Enter);
        assert!(app.inspector.texto().is_none(), "confirmar cierra la edición");
        assert_eq!(app.borrador.valor("rm_root", &Campo::When).as_deref(), Some("has(input.x)"));
        assert!(!app.borrador.doc.cambios().vacio(), "y queda un cambio sin guardar");
        assert!(app.borrador.validez.ok, "{:?}", app.borrador.validez);
    }

    #[test]
    fn el_esc_cancela_la_edicion_del_campo() {
        let (_tmp, mut app) = editor(LOCAL);
        tecla(&mut app, KeyCode::Tab);
        let antes = app.borrador.doc.def().nodes["rm_root"].instructions.clone();
        tecla(&mut app, KeyCode::Char('e'));
        for c in "otra cosa".chars() {
            tecla(&mut app, KeyCode::Char(c));
        }
        tecla(&mut app, KeyCode::Esc);
        assert!(app.inspector.texto().is_none());
        assert_eq!(app.borrador.doc.def().nodes["rm_root"].instructions, antes);
        assert!(app.borrador.doc.cambios().vacio());
    }

    #[test]
    fn un_valor_que_no_vale_deja_la_edicion_abierta_y_lo_dice() {
        let (_tmp, mut app) = editor(LOCAL);
        // It goes down to the `min_confidence` of the `rm_root` node… which is a `rule` and has none: `risks` is used.
        for _ in 0..64 {
            if app.grafo.parada() == Some(&Parada::Nodo("risks".into())) {
                break;
            }
            tecla(&mut app, KeyCode::Right);
        }
        tecla(&mut app, KeyCode::Tab);
        let filas = app.filas();
        let fila = filas.iter().position(|f| f.campo == Campo::MinConfidence).expect("min_confidence");
        for _ in 0..fila {
            tecla(&mut app, KeyCode::Down);
        }
        tecla(&mut app, KeyCode::Char('e'));
        for _ in 0..40 {
            tecla(&mut app, KeyCode::Backspace);
        }
        for c in "mucha".chars() {
            tecla(&mut app, KeyCode::Char(c));
        }
        tecla(&mut app, KeyCode::Enter);
        assert!(app.inspector.texto().is_some(), "un valor que no vale no cierra la edición");
        assert!(app.aviso_banda.as_deref().unwrap().contains("no es un número"), "{:?}", app.aviso_banda);
    }

    #[test]
    fn la_s_guarda_solo_lo_tocado() {
        let (_tmp, mut app) = editor(LOCAL);
        let cuando = "answers.risks.destructive >= 0.6 || answers.risks.exfiltration >= 0.55";
        app.borrador.campo("risks", &Campo::RouteWhen(1), cuando).unwrap();
        tecla(&mut app, KeyCode::Char('s'));

        assert!(app.borrador.doc.cambios().vacio(), "tras guardar no queda nada: {:?}", app.aviso_banda);
        assert!(app.aviso_banda.as_deref().unwrap().contains("1 nodos reescritos"), "{:?}", app.aviso_banda);
        let texto = std::fs::read_to_string(app.borrador.doc.ruta()).unwrap();
        assert!(texto.contains(">= 0.6"), "{texto}");
        assert!(texto.contains("# tool.risk servido en local"), "y los comentarios siguen: {texto}");
    }

    #[test]
    fn con_el_fichero_cambiado_por_debajo_se_ofrece_recargar_o_guardar_aparte() {
        let (_tmp, mut app) = editor(LOCAL);
        app.borrador.campo("risks", &Campo::RouteWhen(1), "answers.risks.destructive >= 0.6").unwrap();
        std::fs::write(app.borrador.doc.ruta(), LOCAL.replace("source: model", "source: model-local")).unwrap();

        tecla(&mut app, KeyCode::Char('s'));
        assert!(matches!(app.aviso, Some(Aviso::Ajena { .. })), "{:?}", app.aviso);
        assert!(
            !std::fs::read_to_string(app.borrador.doc.ruta()).unwrap().contains(">= 0.6"),
            "no se escribe nada mientras haya duda"
        );

        // `g`: saves aside, and the original stays as it was.
        tecla(&mut app, KeyCode::Char('g'));
        let aparte = app.borrador.doc.ruta().with_file_name("tool-risk-local.editado.yaml");
        assert!(aparte.exists(), "se ofrece <nombre>.editado.yaml");
        assert!(std::fs::read_to_string(&aparte).unwrap().contains(">= 0.6"));
        assert!(
            std::fs::read_to_string(app.borrador.doc.ruta()).unwrap().contains("model-local"),
            "el original, intacto"
        );
    }

    #[test]
    fn la_r_recarga_con_confirmacion_y_descarta_el_borrador() {
        let (_tmp, mut app) = editor(LOCAL);
        app.borrador.campo("rm_root", &Campo::Instructions, "otra").unwrap();
        tecla(&mut app, KeyCode::Char('r'));
        assert_eq!(app.aviso, Some(Aviso::Recargar));
        tecla(&mut app, KeyCode::Enter);
        assert!(app.borrador.doc.cambios().vacio(), "el borrador se fue");
        assert_ne!(app.borrador.doc.def().nodes["rm_root"].instructions, "otra");
    }

    #[test]
    fn la_q_avisa_si_hay_cambios_sin_guardar() {
        let (_tmp, mut app) = editor(LOCAL);
        app.borrador.campo("rm_root", &Campo::Instructions, "algo").unwrap();
        tecla(&mut app, KeyCode::Char('q'));
        assert!(!app.salir, "no se sale a la primera");
        assert_eq!(app.aviso, Some(Aviso::Salir));
        tecla(&mut app, KeyCode::Enter);
        assert!(app.salir);
    }

    #[test]
    fn el_menu_de_tipos_anade_el_nodo_elegido() {
        let (_tmp, mut app) = editor(LOCAL);
        tecla(&mut app, KeyCode::Char('a'));
        let menu = app.menu.clone().expect("el menú de tipos");
        assert_eq!(menu.opciones, ["rule", "noul", "choice", "score", "batch"]);
        tecla(&mut app, KeyCode::Down);
        tecla(&mut app, KeyCode::Down);
        tecla(&mut app, KeyCode::Enter);
        assert!(app.menu.is_none());
        assert_eq!(app.borrador.doc.def().nodes["nuevo-1"].ty, NodeType::Choice);
        assert!(!app.borrador.validez.ok, "nace sin cablear, y el parser lo dice");
        assert!(app.aviso_banda.as_deref().unwrap().contains("nuevo-1"), "{:?}", app.aviso_banda);
    }

    #[test]
    fn el_menu_de_destinos_recablea_el_puerto_seleccionado() {
        let (_tmp, mut app) = editor(LOCAL);
        tecla(&mut app, KeyCode::Right);
        assert_eq!(app.grafo.puerto(), Some(("rm_root", "true")));
        tecla(&mut app, KeyCode::Char('c'));
        let menu = app.menu.clone().expect("el menú de destinos");
        assert_eq!(menu.titulo, "recablear rm_root.true");
        let destino = menu.opciones.iter().position(|o| o == "allow").expect("allow es destino");
        for _ in 0..destino {
            tecla(&mut app, KeyCode::Down);
        }
        tecla(&mut app, KeyCode::Enter);
        assert_eq!(app.borrador.doc.def().nodes["rm_root"].out["true"], "allow");
        assert!(app.borrador.validez.ok, "{:?}", app.borrador.validez);
    }

    #[test]
    fn la_c_sin_puerto_seleccionado_lo_dice() {
        let (_tmp, mut app) = editor(LOCAL);
        tecla(&mut app, KeyCode::Char('c'));
        assert!(app.menu.is_none());
        assert!(app.aviso_banda.as_deref().unwrap().contains("selecciona un puerto"), "{:?}", app.aviso_banda);
    }

    /// `s` only saves: destructive confirmations are confirmed by `enter`, which is what the
    /// band announces. With `s` confirming too, pressing "save" in front of "delete risks?"
    /// deleted the node by accident.
    #[test]
    fn la_s_no_confirma_lo_destructivo() {
        let (_tmp, mut app) = editor(LOCAL);
        for destructivo in [Aviso::Salir, Aviso::Recargar, Aviso::Borrar("rm_root".into())] {
            app.aviso = Some(destructivo.clone());
            tecla(&mut app, KeyCode::Char('s'));
            assert_eq!(app.aviso, Some(destructivo.clone()), "`s` no ha de confirmar {destructivo:?}");
            assert!(!app.salir, "y menos aún salir");
            assert!(app.borrador.doc.def().nodes.contains_key("rm_root"), "ni borrar el nodo");
        }
        // And `enter` does confirm: it is still possible to quit.
        app.aviso = Some(Aviso::Salir);
        tecla(&mut app, KeyCode::Enter);
        assert!(app.salir, "enter sí confirma la salida");
    }

    /// Saving aside cannot be D5's back door: an invalid draft does not go to disk, and
    /// the notice stays pending so the user can reload.
    #[test]
    fn guardar_aparte_no_escribe_un_borrador_invalido() {
        let (tmp, mut app) = editor(LOCAL);
        // With the daemon's catalogue, the broken CEL really leaves the draft invalid.
        app.borrador.catalogo(vec!["kev".into()], "kev".into());
        app.borrador.campo("risks", &Campo::RouteWhen(1), "answers.risks.destructive >=").unwrap();
        assert!(!app.borrador.validez.ok, "la CEL quedó rota: {:?}", app.borrador.validez);

        // The file changed underneath: `s` warns, it does not write.
        let ruta = app.borrador.doc.ruta().to_path_buf();
        std::fs::write(&ruta, LOCAL.replace("pipe_sh", "pipe_sh2")).unwrap();
        tecla(&mut app, KeyCode::Char('s'));
        assert!(matches!(app.aviso, Some(Aviso::Ajena { .. })), "{:?}", app.aviso);

        tecla(&mut app, KeyCode::Char('g'));
        let aparte = tmp.0.join("tool-risk-local.editado.yaml");
        assert!(!aparte.exists(), "un borrador inválido no ha de escribirse aparte");
        assert!(app.aviso.is_some(), "y el aviso ha de seguir pendiente para poder recargar");
        assert!(
            app.aviso_banda.as_deref().is_some_and(|m| m.contains("no se guarda")),
            "y ha de decir por qué: {:?}",
            app.aviso_banda
        );
    }

    #[test]
    fn el_esc_suelta_el_aviso_y_el_menu_sin_hacer_nada() {
        let (_tmp, mut app) = editor(LOCAL);
        app.borrador.campo("rm_root", &Campo::Instructions, "otra").unwrap();
        tecla(&mut app, KeyCode::Char('r'));
        assert_eq!(app.aviso, Some(Aviso::Recargar));
        tecla(&mut app, KeyCode::Esc);
        assert_eq!(app.aviso, None, "el aviso se suelta");
        assert_eq!(app.borrador.doc.def().nodes["rm_root"].instructions, "otra", "y el borrador sigue donde estaba");
        // And the house responds again: the notice does not leave the editor locked in.
        tecla(&mut app, KeyCode::Tab);
        assert_eq!(app.foco, Foco::Inspector);

        tecla(&mut app, KeyCode::Char('a'));
        assert!(app.menu.is_some());
        tecla(&mut app, KeyCode::Esc);
        assert!(app.menu.is_none(), "el menú se cierra sin elegir");
        assert!(!app.borrador.doc.def().nodes.contains_key("nuevo-1"), "y no añade nada");
    }

    #[test]
    fn el_borrado_pide_confirmacion_y_lista_los_puertos_que_lo_referencian() {
        let (_tmp, mut app) = editor(LOCAL);
        for _ in 0..64 {
            if app.grafo.parada() == Some(&Parada::Nodo("risks".into())) {
                break;
            }
            tecla(&mut app, KeyCode::Right);
        }
        tecla(&mut app, KeyCode::Char('d'));
        assert_eq!(app.aviso, Some(Aviso::Borrar("risks".into())));
        tecla(&mut app, KeyCode::Enter);
        assert_eq!(app.aviso, None);
        assert!(app.aviso_banda.as_deref().unwrap().contains("pipe_sh.false"), "{:?}", app.aviso_banda);
        assert!(app.borrador.doc.def().nodes.contains_key("risks"), "y no se ha borrado");
    }
}
