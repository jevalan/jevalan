---
title: Comandos
description: Los comandos de la TUI, el daemon, el editor de flujos, los hooks, el registro sombra y el laboratorio.
---

| Comando | Uso |
|---|---|
| `jevalan tui --demo` | Abre la TUI con un daemon, servicios simulados y tráfico simulado. No necesita claves. |
| `jevalan tui --fake` | Abre la TUI con una secuencia determinista, sin daemon ni backends. |
| `jevalan tui` | Se conecta a un daemon que responda, o arranca uno contra tus backends reales. |
| `jevalan serve --config config.yaml` | Arranca el daemon local. |
| `jevalan edit config/flows/<flujo>.yaml` | Edita un flujo como borrador. |
| `jevalan hook claude-code [--mode shadow\|enforce]` | El hook `PreToolUse` para Claude Code. |
| `jevalan hook crush [--mode shadow\|enforce]` | El hook para Crush. |
| `jevalan hook run --client <nombre> --dialect <claude\|crush>` | El adaptador de hook genérico. |
| `jevalan shadow report` | Un informe en Markdown sobre el registro sombra. |
| `jevalan shadow fixtures` | Convierte decisiones reales en fixtures del laboratorio. |
| `jevalan mcp [--allow-live]` | Servidor MCP por stdio: expone las capacidades del hub como herramientas. Ver [Servidor MCP](/servidor-mcp/). |
| `jevalan lab check` | Valida los fixtures del laboratorio sin conexión. |
| `jevalan lab offline` | Reproduce los fixtures sin conexión. |
| `jevalan lab record` | Llama a backends reales y graba cassettes, dentro de un presupuesto. |
| `jevalan lab live` | Mide backends reales con repeticiones, dentro de un presupuesto. |
| `jevalan --version` | Muestra la versión instalada. |

Ejecuta `jevalan <comando> --help` para ver todas las opciones de un comando.

## Opciones de los demás comandos

| Comando | Opciones |
|---|---|
| `serve` | `--config` (sin él, `JEVALAN_CONFIG` o los valores por defecto). |
| `edit` | `--url` y `--token` del daemon, `--no-events` (no escuchar el bus de eventos) y `--signals-dir` (por defecto `./config/signals`). |
| `mcp` | `--addr` y `--token` del daemon, y `--allow-live`. |
| `hook` | `--mode shadow\|enforce` (por defecto `shadow`); en `hook run`, además `--client` (por defecto `unknown`) y `--dialect claude\|crush`. |
| `shadow report` | `--log`, `--since` (RFC 3339 o `AAAA-MM-DD`, en UTC), `--out` y `--with-inputs`. |
| `shadow fixtures` | `--log`, `--since`, `--lab-dir` (por defecto `lab-fixtures-batch`) y `--max`. |
| `lab check`, `lab offline`, `lab record`, `lab live` | `--lab-dir` (por defecto `config/lab`) y `--config`. Todos menos `check` aceptan también una capacidad como argumento y `--fixture`. `record` y `live` añaden `--target` y `--yes`; `record`, `--force`; `live`, `--repeat`. |

## Opciones de la interfaz de terminal

| Flag | Significado | Por defecto |
|---|---|---|
| `--url` | El WebSocket de eventos del daemon. | `ws://127.0.0.1:8080/events` |
| `--token` | El token del daemon (también `JEVALAN_TOKEN`). | ninguno |
| `--fx` | Efectos visuales: `off`, `events` o `full` (también `JEVALAN_FX`). | `events` |
| `--fake` | Alimenta la TUI con trazas simuladas. | desactivado |
| `--fake-rate`, `--fake-seed` | Peticiones simuladas por segundo, y una semilla para ejecuciones repetibles. | `2`, aleatoria |
| `--demo` | Backends simulados y tráfico simulado. | desactivado |
| `--rate` | Peticiones por segundo del tráfico de `--demo`. | `2` |
| `--config` | Fichero de configuración del daemon que la TUI arranca, si hace falta. | ninguno |

### Teclas

| Tecla | Acción |
|---|---|
| `q` | Salir. |
| `?` | Ayuda. |
| `p` | Pausar o reanudar. |
| `f` | Cambiar de efectos: off, events, full. |
| `esc` | Volver. |
| `tab` | Cambiar el foco entre la topología y el feed. |
| `enter` | Abrir el flujo (en un puerto de la topología) o la traza (en una fila del feed). |
| flechas o `hjkl` | Mover la vista de la topología o de un flujo. En el feed, `↑` `↓` eligen la fila; en una traza, desplazan. |
| `+` (o `=`) `-` `0` | Acercar, alejar, ajustar a la vista. |
| ratón | Arrastrar mueve la vista y la rueda hace zoom, en la topología y en un flujo. |
| `n` `N` | Nodo siguiente o anterior en la topología. |
| `t` | En una traza, alterna entre el camino de nodos y la línea de tiempo de eventos. |
| `H` | Vista en vivo de las decisiones del hook. Dentro, `↑` `↓` o `jk` se mueven y `g` `G` (o `Inicio` `Fin`) saltan a la primera o a la última. |
| `x` | En `--demo`, con el foco en la topología, tumba o recupera el backend seleccionado. |

## Teclas del editor de flujos

`jevalan edit` es un editor de terminal sobre un fichero de flujo. Trabaja sobre un borrador: nunca publica, nunca toca el registro del daemon, y un borrador que no valida nunca se escribe en disco.

| Tecla | Acción |
|---|---|
| `tab` | Moverse entre el grafo, el inspector y la banda inferior. |
| flechas o `hjkl` | Mover la selección. |
| `e` | Editar el campo seleccionado. `enter` lo confirma en el borrador. |
| `a` | Añadir un nodo: `rule`, `noul`, `choice`, `score` o `batch`. |
| `d` | Borrar el nodo seleccionado, con confirmación. |
| `c` | Recablear el puerto seleccionado. |
| `s` | Guardar por parches, conservando una copia `.bak`. Los comentarios y el orden sobreviven. |
| `g` | Guardar aparte como `<nombre>.editado.yaml` cuando el fichero cambió por debajo. |
| `r` | Recargar desde disco, descartando el borrador. |
| `q` | Salir, avisando si hay cambios sin guardar. |
| `esc` | Cancelar la edición, el menú o la confirmación en curso. |

`config/flows/tool-risk-local.yaml` participa en las decisiones del hook. Practica sobre una copia antes de editarlo.
