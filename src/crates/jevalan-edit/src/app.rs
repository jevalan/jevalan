//! Estado del editor: el foco, el documento abierto, su grafo y lo que se está confirmando.

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

/// Las tres zonas de la pantalla, en el orden en que las recorre `tab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foco {
    Grafo,
    Inspector,
    Banda,
}

/// Lo que el editor está pidiendo confirmar.
#[derive(Debug, Clone, PartialEq)]
pub enum Aviso {
    /// Salir con cambios sin guardar.
    Salir,
    /// Recargar desde el disco, descartando el borrador.
    Recargar,
    /// Borrar el nodo nombrado.
    Borrar(String),
    /// El fichero cambió por debajo: recargar con `r` o guardar aparte con `g`.
    Ajena { ruta: PathBuf },
}

/// Qué se hace con la opción elegida en el menú de una línea.
#[derive(Debug, Clone, PartialEq)]
pub enum Fin {
    /// Añadir un nodo del tipo elegido, por el orden de `TIPOS`.
    AnadirTipo,
    /// Recablear este puerto al destino elegido.
    Cablear { nodo: String, puerto: String },
}

/// Menú de una línea: lo que se elige con las flechas y se confirma con `enter`.
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

/// El estado entero del editor.
pub struct App {
    pub borrador: Borrador,
    pub grafo: Grafo,
    pub inspector: Inspector,
    pub telemetria: Telemetria,
    /// El pintor del grafo, que se reutiliza tal cual del dashboard.
    pub pintor: FlowGraph,
    pub foco: Foco,
    /// El menú de una línea abierto (`a` tipos, `c` destinos).
    pub menu: Option<Menu>,
    pub aviso: Option<Aviso>,
    /// Aviso efímero de la banda inferior.
    pub aviso_banda: Option<String>,
    pub salir: bool,
    pub redibujar: bool,
    pub ahora: Instant,
}

impl App {
    /// Arma el editor con el documento ya abierto y su telemetría.
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

    /// El nombre y la definición del nodo seleccionado, si la parada es un nodo.
    pub fn nodo(&self) -> Option<(&str, &NodeDef)> {
        let nombre = self.grafo.nodo()?;
        self.borrador.doc.def().nodes.get(nombre).map(|d| (nombre, d))
    }

    /// Las filas del inspector para el nodo seleccionado.
    pub fn filas(&self) -> Vec<Fila> {
        self.nodo().map_or_else(Vec::new, |(_, d)| crate::inspector::filas(d))
    }

    /// Rehace el grafo con el borrador y ajusta el cursor del inspector.
    pub fn rehacer(&mut self) {
        self.grafo.rehacer(&self.borrador.nombre(), self.borrador.doc.def());
        self.inspector.ajustar(self.filas().len());
        self.redibujar = true;
    }

    /// Una tecla. Las teclas de la casa valen desde cualquier zona, como en la TUI.
    pub fn tecla(&mut self, k: KeyEvent) {
        if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        self.redibujar = true;
        // El orden manda: primero lo que se está escribiendo, luego el menú, luego lo que se confirma.
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

    /// Las teclas mientras se escribe un campo: `enter` lo confirma en el borrador, `esc` lo cancela.
    fn tecla_edicion(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => self.confirmar_campo(),
            KeyCode::Esc => self.inspector.cancelar(),
            KeyCode::Backspace => self.inspector.borrar(),
            KeyCode::Char(c) => self.inspector.escribir(c),
            _ => {}
        }
    }

    /// Confirma el texto en curso en el borrador, nunca en el disco.
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
            // Un valor que no vale deja la edición abierta para poder corregirlo.
            Err(e) => self.aviso_banda = Some(e),
        }
    }

    /// En el grafo, las flechas mueven la selección por nodos y por aristas.
    fn tecla_grafo(&mut self, code: KeyCode) {
        match code {
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j') | KeyCode::Char('l') => self.grafo.siguiente(),
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => self.grafo.anterior(),
            _ => {}
        }
        self.inspector.ajustar(self.filas().len());
    }

    /// En el inspector, las flechas recorren sus filas y `e` entra en el campo seleccionado.
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

    /// `s`: guarda si el fichero no cambió por debajo; si cambió, ofrece recargar o guardar aparte.
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

    /// Pide una confirmación sencilla.
    fn solo_pedir(&mut self, aviso: Aviso) {
        self.aviso = Some(aviso);
    }

    /// `a`: añade un nodo; abre el menú de tipos.
    fn pedir_anadir(&mut self) {
        let opciones: Vec<String> = TIPOS.iter().map(|t| t.as_str().to_string()).collect();
        self.menu = Some(Menu::nuevo("añadir nodo".into(), opciones, Fin::AnadirTipo));
    }

    /// `d`: borra el nodo seleccionado, con confirmación.
    fn pedir_borrar(&mut self) {
        match self.grafo.nodo() {
            Some(nodo) => self.aviso = Some(Aviso::Borrar(nodo.to_string())),
            None => self.aviso_banda = Some("selecciona un nodo para borrarlo".into()),
        }
    }

    /// `c`: recablea el puerto seleccionado; abre el menú de destinos.
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

    /// Las teclas del menú de una línea. `esc` lo cierra sin elegir nada: un menú que no se pudiera
    /// soltar dejaría al usuario encerrado en él.
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

    /// Aplica la opción elegida en el menú.
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

    /// `tab` recorre las tres zonas: grafo, inspector y banda.
    fn cambiar_foco(&mut self) {
        self.foco = match self.foco {
            Foco::Grafo => Foco::Inspector,
            Foco::Inspector => Foco::Banda,
            Foco::Banda => Foco::Grafo,
        };
    }

    /// `q`: se sale, avisando si hay cambios sin guardar (D5).
    fn pedir_salir(&mut self) {
        let hay_cambios = !self.borrador.doc.cambios().vacio() && self.borrador.doc.solo_lectura().is_none();
        if hay_cambios {
            self.aviso = Some(Aviso::Salir);
        } else {
            self.salir = true;
        }
    }

    /// Una tecla con confirmación pendiente. `esc` cancela siempre; `enter` o `s` confirman las que
    /// se confirman así; el cambio por debajo se atiende con `r` o con `g`.
    fn confirmar(&mut self, code: KeyCode) {
        let Some(aviso) = self.aviso.take() else { return };
        match (aviso, code) {
            // `esc` cancela siempre: el aviso se suelta sin hacer nada. Sin esta rama, el comodín de
            // abajo volvería a armarlo y la confirmación no se podría abandonar.
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
            // El fichero cambió por debajo: `r` recarga, `g` guarda aparte, y nada se escribe solo.
            (Aviso::Ajena { .. }, KeyCode::Char('r')) => match self.borrador.recargar() {
                Ok(()) => {
                    self.aviso_banda = Some("recargado del disco; el borrador se descartó".into());
                    self.rehacer();
                }
                Err(e) => self.aviso_banda = Some(e),
            },
            (Aviso::Ajena { ruta }, KeyCode::Char('g')) => {
                let aparte = editado(&ruta);
                // El veto de D5 vive en el borrador, no aquí: un flujo inválido no se escribe aparte
                // ni se escribe solo. Si se niega, el aviso sigue puesto para poder pulsar `r`.
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

    /// Recoge lo que llegue del bus y, si el catálogo de backends cambió, revalida.
    pub fn latir(&mut self) {
        self.ahora = Instant::now();
        let antes = self.telemetria.backends.clone();
        if self.telemetria.latir(self.ahora) {
            self.redibujar = true;
            if self.telemetria.backends != antes {
                // El backend por defecto del daemon no viaja en el evento, así que no se finge uno:
                // sin él la validación local no comprueba los backends, y lo dice (spec §8).
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

/// El fichero aparte que se ofrece cuando el original cambió por debajo: `<nombre>.editado.yaml`.
fn editado(ruta: &std::path::Path) -> PathBuf {
    let tallo = ruta.file_stem().and_then(|n| n.to_str()).unwrap_or("flujo");
    ruta.with_file_name(format!("{tallo}.editado.yaml"))
}

// crates/jevalan-edit/src/app.rs — al final del fichero (las pruebas de la pantalla van aquí: usan el editor entero)
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

    /// Directorio temporal que se borra al soltarse.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            // Cada directorio necesita nombre propio: las pruebas corren en paralelo y comparten
            // `temp_dir()`; con un nombre fijo, el `remove_dir_all` de una borra el de otra.
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

    /// Un editor con el flujo local abierto y sin bus.
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
        // El inspector sigue mirando el mismo nodo, porque la parada es un puerto suyo.
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

        // El editor escribe al final: se borra la expresión y se escribe otra.
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
        // Se baja hasta `min_confidence` del nodo `rm_root`… que es un `rule` y no tiene: se usa `risks`.
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

        // `g`: se guarda aparte, y el original queda como estaba.
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

    /// La `s` solo guarda: las confirmaciones destructivas las confirma `enter`, que es lo que la
    /// banda anuncia. Con `s` confirmando también, pulsar «guardar» delante de «¿borrar risks?»
    /// borraba el nodo sin querer.
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
        // Y `enter` sí confirma: se sigue pudiendo salir.
        app.aviso = Some(Aviso::Salir);
        tecla(&mut app, KeyCode::Enter);
        assert!(app.salir, "enter sí confirma la salida");
    }

    /// Guardar aparte no puede ser la puerta de atrás de D5: un borrador inválido no va al disco, y
    /// el aviso sigue pendiente para que el usuario pueda recargar.
    #[test]
    fn guardar_aparte_no_escribe_un_borrador_invalido() {
        let (tmp, mut app) = editor(LOCAL);
        // Con el catálogo del daemon, la CEL rota deja el borrador inválido de verdad.
        app.borrador.catalogo(vec!["kev".into()], "kev".into());
        app.borrador.campo("risks", &Campo::RouteWhen(1), "answers.risks.destructive >=").unwrap();
        assert!(!app.borrador.validez.ok, "la CEL quedó rota: {:?}", app.borrador.validez);

        // El fichero cambió por debajo: `s` avisa, no escribe.
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
        // Y la casa vuelve a responder: el aviso no deja el editor encerrado.
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
