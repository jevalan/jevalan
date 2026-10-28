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

## Opciones de la interfaz de terminal

| Flag | Significado | Por defecto |
|---|---|---|
| `--url` | El WebSocket de eventos del daemon. | `ws://127.0.0.1:8080/events` |
| `--token` | El token del daemon (también `JEVALAN_TOKEN`). | ninguno |
| `--fx` | Efectos visuales: `off`, `events` o `full` (también `JEVALAN_FX`). | `events` |
| `--fake` | Alimenta la TUI con trazas simuladas. | desactivado |
| `--fake-rate`, `--fake-seed` | Peticiones simuladas por segundo, y una semilla para ejecuciones repetibles. | `2`, aleatoria |
| `--demo` | Backends simulados y tráfico simulado. | desactivado |
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
| flechas o `hjkl` | Mover la vista. |
| `+` `-` `0` | Acercar, alejar, ajustar a la vista. |
| `n` `N` | Nodo siguiente o anterior. |
| `t` | En una traza, alterna entre el camino de nodos y la línea de tiempo de eventos. |
| `H` | Vista en vivo de las decisiones del hook. |
| `x` | En `--demo`, tumba o recupera el backend seleccionado. |

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
