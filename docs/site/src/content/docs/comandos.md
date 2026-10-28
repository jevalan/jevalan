---
title: Comandos
description: Comandos habituales para la TUI, el daemon, los flujos y el laboratorio.
---

| Comando | Uso |
|---|---|
| `jevalan tui --demo` | Abre la TUI con daemon, servicios mock y tráfico simulado. |
| `jevalan tui --fake` | Abre la TUI con una secuencia determinista, sin daemon ni backends. |
| `jevalan serve --config config.yaml` | Arranca el daemon local. |
| `jevalan edit config/flows/<flujo>.yaml` | Edita un flujo como borrador. |
| `jevalan lab check` | Valida los fixtures del laboratorio sin conexión. |
| `jevalan lab offline` | Reproduce los fixtures offline. |
| `jevalan --version` | Muestra la versión instalada. |

`config/flows/tool-risk-local.yaml` participa en la decisión del hook. Practica con una copia si vas a editarlo.

Para ver todas las opciones de un comando, ejecuta `jevalan <comando> --help`.
