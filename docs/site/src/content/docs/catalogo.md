---
title: Catálogo de capacidades
description: Qué capacidades vienen con Jevalan, qué pregunta responde cada una, quién las consume y qué hacer cuando no hay respuesta.
---

Esta página es el inventario. Para el mecanismo (cómo se pide una capacidad, cómo se define, qué estados tiene una respuesta) mira [Capacidades y hub](/capacidades/). Aquí está lo que ya existe y lo que no.

Cada capacidad figura con uno de estos estados:

| Estado | Significado |
|---|---|
| **Incluida** | Tiene manifiesto, esquemas, flujo o proveedor y fixtures del [laboratorio](/laboratorio/) en el repositorio. |
| **Flujo de ejemplo** | Existe como flujo (`POST /v1/decide/{flujo}`), pero no está registrada como capacidad en el hub. |
| **En estudio** | Idea sin implementar y sin fecha. |

## `tool.risk` (incluida)

**Pregunta:** ¿es arriesgada esta llamada a una herramienta? Responde `allow`, `ask` o `deny`.

| | |
|---|---|
| Entrada | `tool_name` y `tool_input` (obligatorios); `cwd`, `session_id`, `tool_use_id` y `transcript_path` (opcionales). |
| Decisión | `verdict` (`allow`, `ask`, `deny`), `reason`, `source` y, si hubo modelo, `probabilities`. |
| Plazo | 5000 ms por defecto. |
| Quién la sirve | `config/capabilities-local/tool.risk.yaml` la decide con el flujo `tool-risk-local`, todo en local. `config/capabilities/tool.risk.yaml` la delega en una herramienta externa mediante un proveedor `exec`, que exige `hub.allow_exec: true`. |
| Quién la consume | Los [hooks](/integraciones/) de Claude Code y Crush, y la herramienta `tool_risk` del [servidor MCP](/servidor-mcp/). |
| Sin respuesta | El hook responde `ask`, nunca un `allow` a ciegas. |

El flujo local aplica primero dos reglas fijas (borrado recursivo de la raíz o del hogar, y descarga canalizada a un intérprete) y solo si ninguna salta pregunta al modelo cuatro cosas en una sola llamada: destrucción, exfiltración, privilegios y exposición de secretos. Un riesgo de 0.85 o más es `deny`; de 0.55 o más, `ask`; por debajo, `allow`. Las reglas y las cuatro preguntas viven en `config/signals/`, y cualquier flujo puede reutilizarlas.

## `agent.wakeup` (incluida)

**Pregunta:** un agente duerme esperando algo, ha llegado un evento: ¿merece la pena despertarlo?

| | |
|---|---|
| Entrada | `waiting_for` y `skipped` (obligatorios); `event`, `observation`, `from_user` y `max_skips` (opcionales). |
| Decisión | `wake` (booleano), `reason` y, si hubo modelo, `probability`. |
| Plazo | 3000 ms por defecto. |
| Quién la sirve | El flujo `agent-wakeup`, a través de `config/capabilities/agent.wakeup.yaml`. |
| Quién la consume | Llamadas a `POST /v1/hub` y la herramienta `agent_wakeup` del servidor MCP. |
| Sin respuesta | El propio flujo despierta al agente (`reason: error`) si el modelo falla: prefiere un despertar de más a un agente dormido para siempre. |

Las razones posibles son `user-message`, `nothing-to-judge`, `skip-limit`, `judged`, `unsure` y `error`. Las tres primeras son reglas deterministas y no consultan ningún modelo: un mensaje del usuario siempre despierta, un aviso sin evento ni observación también, y tras `max_skips` omisiones seguidas (10 por defecto) se despierta igualmente. Solo el resto llega al modelo.

## Flujos de ejemplo

Estos dos flujos viven en `config/flows/` y se ejecutan con `POST /v1/decide/{flujo}`. No son capacidades del hub: no tienen esquema de entrada ni de decisión, así que el hub no los valida ni los expone por MCP. Convertir uno en capacidad es escribirle un manifiesto, como se explica más abajo.

| Flujo | Qué decide | Resultado |
|---|---|---|
| `agent-guard` | Un guardián de comandos de agentes: ¿destruye datos fuera del proyecto? ¿qué tipo de tarea es? ¿cuánto riesgo tiene? | `allow`, `deny`, `ask-user` o `abstain`. |
| `model-route` | Qué modelo y qué esfuerzo de razonamiento merece una tarea, según su dificultad y si toca seguridad, producción o dinero. | `route`, con `model` y `effort`. |

El ejemplo 4 de [Ejemplos](/ejemplos/) recorre `model-route`.

## En estudio

Nada de esto está implementado ni tiene fecha. Se anota para que quede claro qué es ya producto y qué no.

| Idea | Pregunta | Por qué encaja |
|---|---|---|
| `agent.stop` | ¿Puede este agente dar su trabajo por terminado? | Mismo patrón que `agent.wakeup`: reglas fijas (¿pasaron los tests?) y un modelo solo para lo dudoso. |
| `model.route` | ¿Qué modelo y esfuerzo merece esta tarea? | Ya existe como flujo; le faltaría manifiesto y esquemas. |

## Añadir una capacidad

1. Escribe el flujo en `config/flows/` (con [`jevalan edit`](/comandos/), no a mano; mira [Flujos](/flujos/)).
2. Escribe los dos esquemas JSON en `schemas/` junto al manifiesto: `<capacidad>.input.json` y `<capacidad>.decision.json`.
3. Escribe el manifiesto `<capacidad>.yaml` en `hub.capabilities_dir`, con `provider: { id: jevalan-flow, flow: <flujo> }`.
4. Añade fixtures en `config/lab/fixtures/<capacidad>/` y pasa `jevalan lab check` y `jevalan lab offline`.
5. Decide el comportamiento sin respuesta del consumidor **antes** de enchufarlo (mira la [guía](/guia-decisiones/)).

Si algo falla al cargar, `GET /v1/hub/capabilities` lo lista bajo `invalid` con el motivo; una capacidad de tipo `flow` también queda inválida mientras su flujo no exista.
