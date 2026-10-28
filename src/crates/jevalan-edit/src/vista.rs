//! Pantalla del editor: cabecera, grafo, inspector y banda inferior.

use crate::app::{App, Aviso, Foco};
use crate::grafo::Parada;
use crate::telemetria::Estado;
use jevalan_flow::NodeType;
use jevalan_tui::flow_layout::FlowNodeId;
use jevalan_tui::theme;
use jevalan_tui::widgets::flow_graph;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

/// Lo que el editor necesita para poder pintar; por debajo de esto, ni lo intenta, como la TUI.
pub const MIN_ANCHO: u16 = 76;
pub const MIN_ALTO: u16 = 20;

/// Pinta la pantalla entera: cabecera, grafo, inspector y banda inferior.
pub fn dibujar(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.width < MIN_ANCHO || area.height < MIN_ALTO {
        let aviso = Paragraph::new("terminal demasiado pequeño").alignment(Alignment::Center);
        f.render_widget(aviso, area);
        return;
    }
    // El binding no puede llamarse `banda`: sombrearía a la función homónima de este fichero.
    let [cabecera, grafo, inspector, franja_baja] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(alto_inspector(app, area)),
        Constraint::Length(1),
    ])
    .areas(area);
    pinta_cabecera(f, app, cabecera);
    pinta_grafo(f, app, grafo);
    pinta_inspector(f, app, inspector);
    if app.menu.is_some() {
        pinta_menu(f, app, franja_baja);
    } else {
        f.render_widget(Paragraph::new(banda(app)).style(estilo_banda(app)), franja_baja);
    }
}

/// El menú de una línea: el título y sus opciones, con la elegida marcada.
fn pinta_menu(f: &mut Frame, app: &App, area: Rect) {
    let Some(menu) = &app.menu else { return };
    let opciones: Vec<Span> = menu
        .opciones
        .iter()
        .enumerate()
        .flat_map(|(i, o)| {
            let estilo = if i == menu.cursor { theme::accent() } else { theme::dim() };
            [Span::styled(format!(" {} ", o), estilo), Span::styled("·", theme::dim())]
        })
        .collect();
    let mut spans = vec![Span::styled(format!("{}: ", menu.titulo), theme::title())];
    spans.extend(opciones);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// El alto del inspector: sus líneas, más el marco; nunca más de media pantalla.
fn alto_inspector(app: &App, area: Rect) -> u16 {
    (lineas_inspector(app).len() as u16 + 2).clamp(4, area.height / 2)
}

/// La franja de arriba: el fichero a la izquierda y el estado del borrador a la derecha.
fn pinta_cabecera(f: &mut Frame, app: &App, area: Rect) {
    let izquierda = format!(" flujo: {} ", app.borrador.nombre());
    let cambios = app.borrador.doc.cambios().total();
    let derecha = if app.borrador.doc.solo_lectura().is_some() {
        "solo lectura: el YAML no parsea ".to_string()
    } else if cambios == 0 {
        "sin cambios ".to_string()
    } else {
        format!("borrador: {cambios} cambios sin guardar ")
    };
    f.render_widget(franja(&izquierda, &derecha, area.width), area);
}

/// El grafo, dentro de un marco; si no hay nada que pintar, lo dice en vez de inventarlo.
fn pinta_grafo(f: &mut Frame, app: &mut App, area: Rect) {
    let marco = Block::bordered()
        .title(Span::styled(format!(" {} ", app.borrador.nombre()), theme::dim()))
        .border_style(estilo_foco(app.foco == Foco::Grafo));
    let interior = marco.inner(area);
    f.render_widget(marco, area);
    if app.grafo.vacio() {
        f.render_widget(Paragraph::new("sin grafo que pintar").style(theme::dim()), interior);
        return;
    }
    // Si el validador señala un nodo y el cursor está en otro sitio, manda el culpable: un borrador
    // inválido ha de verse en el grafo, no solo en la banda (spec §3). `marcado` filtra el nombre,
    // que a veces nombra una regla (`risks.route`) y no un nodo.
    let def = app.borrador.doc.def();
    let culpable = app.borrador.validez.nodo.clone();
    let resaltado =
        app.grafo.marcado(def, culpable.as_deref()).map(|nombre| flow_graph::caja(&FlowNodeId::Node(nombre)));
    app.pintor.select(resaltado);
    let flujo = app.grafo.flujo.clone();
    app.pintor.render(&flujo, &app.telemetria.activity, app.ahora, interior, f.buffer_mut());
}

/// El inspector del nodo seleccionado: sus campos editables y, en un `batch`, sus preguntas.
fn pinta_inspector(f: &mut Frame, app: &mut App, area: Rect) {
    let (rotulo, leyenda) = match app.nodo() {
        Some((nombre, d)) => (format!(" nodo: {nombre} "), format!("({}) ", d.ty.as_str())),
        None => (" selección ".to_string(), "(sin nodo) ".to_string()),
    };
    let marco = Block::bordered()
        .title(Line::from(vec![Span::styled(rotulo, theme::title()), Span::styled(leyenda, theme::dim())]))
        .border_style(estilo_foco(app.foco == Foco::Inspector));
    let interior = marco.inner(area);
    f.render_widget(marco, area);
    f.render_widget(Paragraph::new(lineas_inspector(app)), interior);
}

/// Las líneas del inspector: una por fila, con la seleccionada marcada, y las preguntas de un `batch`.
fn lineas_inspector(app: &App) -> Vec<Line<'static>> {
    let filas = app.filas();
    if filas.is_empty() {
        return vec![Line::styled("selecciona un nodo en el grafo", theme::dim())];
    }
    let ancho_etiqueta = filas.iter().map(|f| f.etiqueta.chars().count()).max().unwrap_or(0).min(20);
    let mut lineas: Vec<Line<'static>> = filas
        .iter()
        .enumerate()
        .map(|(i, fila)| {
            let seleccionada = i == app.inspector.cursor;
            let editando = seleccionada && app.inspector.texto().is_some();
            let valor =
                if editando { app.inspector.texto().unwrap_or_default().to_string() } else { fila.valor.clone() };
            let estilo = if seleccionada { theme::accent() } else { theme::text() };
            Line::from(vec![
                Span::styled(if seleccionada { "▶ " } else { "  " }, theme::accent()),
                Span::styled(format!("{:<ancho$} ", fila.etiqueta, ancho = ancho_etiqueta), theme::dim()),
                Span::styled(recorta(&valor, 80), estilo),
            ])
        })
        .collect();
    // Un `batch` enseña sus preguntas: se inspeccionan, y quien las juzga es el parser.
    if let Some((_, d)) = app.nodo()
        && d.ty == NodeType::Batch
    {
        let preguntas: Vec<&str> = d.questions.keys().map(String::as_str).collect();
        let salida = |o: Option<&String>| o.map_or("—".to_string(), |t| t.clone());
        lineas.push(Line::styled(format!("preguntas {}", preguntas.join(" · ")), theme::dim()));
        lineas.push(Line::styled(
            format!(
                "route {} reglas · else: {} · error: {}",
                d.route.len(),
                salida(d.else_branch.as_ref()),
                salida(d.out.get("error"))
            ),
            theme::dim(),
        ));
    }
    lineas
}

/// La banda inferior: validación, tamaño del borrador y lo último que se supo del daemon.
pub fn banda(app: &App) -> String {
    let estado = if app.borrador.doc.solo_lectura().is_some() {
        "solo lectura: el YAML no parsea".to_string()
    } else if app.borrador.validez.ok {
        "válido".to_string()
    } else {
        format!("inválido: {}", app.borrador.validez.error.clone().unwrap_or_default())
    };
    let nodos = app.borrador.doc.def().nodes.len();
    let cambios = app.borrador.doc.cambios().total();
    let publicacion = if cambios == 0 {
        "borrador sin publicar".to_string()
    } else {
        format!("borrador sin publicar: {cambios} cambios")
    };
    let bus = match (&app.telemetria.estado, &app.telemetria.ultimo) {
        (Estado::Apagado, _) => "sin telemetría".to_string(),
        (Estado::Conectado, Some(ultimo)) => format!("último evento: {ultimo}"),
        (Estado::Conectado, None) => "bus conectado".to_string(),
        (Estado::Conectando, _) => "bus: conectando".to_string(),
        (Estado::Caido(motivo), _) => format!("bus caído: {motivo}"),
    };
    let parada = match app.grafo.parada() {
        Some(Parada::Puerto { nodo, puerto }) => format!("puerto {nodo}.{puerto} · "),
        _ => String::new(),
    };
    let avisos: Vec<String> =
        [app.aviso_banda.clone(), app.borrador.aviso_validacion.clone()].into_iter().flatten().collect();
    let aviso = if avisos.is_empty() { String::new() } else { format!("{} · ", avisos.join(" · ")) };
    let confirmacion = match &app.aviso {
        Some(Aviso::Salir) => "¿salir con cambios sin guardar? (enter) · ".to_string(),
        Some(Aviso::Recargar) => "¿recargar del disco y descartar el borrador? (enter) · ".to_string(),
        Some(Aviso::Borrar(nodo)) => format!("¿borrar {nodo}? (enter) · "),
        Some(Aviso::Ajena { .. }) => {
            "el fichero cambió por debajo: (r) recargar, (g) guardar aparte, (esc) volver · ".to_string()
        }
        None => String::new(),
    };
    format!("{confirmacion}{aviso}{parada}{estado} · {nodos} nodos · {publicacion} · {bus}")
}

/// El color del marco de una zona: el acento cuando tiene el foco.
fn estilo_foco(con_foco: bool) -> Style {
    if con_foco { theme::accent() } else { theme::dim() }
}

/// La banda se pinta en rojo cuando el borrador no vale.
fn estilo_banda(app: &App) -> Style {
    if app.borrador.doc.solo_lectura().is_some() || !app.borrador.validez.ok { theme::error() } else { theme::text() }
}

/// Una línea con el rótulo a la izquierda, el aviso a la derecha y el centro lleno de raya.
fn franja(izquierda: &str, derecha: &str, ancho: u16) -> Paragraph<'static> {
    let relleno = (ancho as usize).saturating_sub(izquierda.chars().count() + derecha.chars().count());
    Paragraph::new(Line::from(vec![
        Span::styled(izquierda.to_string(), theme::title()),
        Span::styled("─".repeat(relleno), theme::dim()),
        Span::styled(derecha.to_string(), theme::dim()),
    ]))
}

/// Recorta a `n` caracteres, con puntos suspensivos si no cabe.
fn recorta(texto: &str, n: usize) -> String {
    if texto.chars().count() <= n {
        texto.to_string()
    } else {
        texto.chars().take(n.saturating_sub(1)).chain(['…']).collect()
    }
}
