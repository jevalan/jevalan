# Arquitectura de jevalan

Este documento explica, con diagramas, cómo encajan las piezas de jevalan y por qué.

## 1. Vista general

jevalan es un **hub local de decisiones**. Los agentes (Claude Code, Crush, scripts…) le preguntan; jevalan decide —o hace que decida un especialista— y devuelve una respuesta tipada. Nunca ejecuta el efecto: eso queda en manos de quien pregunta.

```
            ┌───────────── agentes, hooks, scripts ─────────────┐
            │                                                    │
   POST /{backend}/v1/systemone   POST /v1/decide/{flujo}   POST /v1/hub
   (pregunta System One cruda)    (un flujo concreto)       (por capacidad, jev-hub/0.1)
            │                          │                         │
╔═══════════▼══════════════════════════▼═════════════════════════▼══════════════╗
║                                   JEVALAN                                   ║
║                                                                             ║
║   passthrough ──────────────┐    motor de flujos ◄──────── registro de      ║
║   (reenvía sin tocar)       │    (rule · noul · choice ·   capacidades      ║
║                             │     score)                   config/capabilities/    ║
║                             │         │                    ├─ flow ─────────╢──► flujos de config/flows/
║                             │         │                    ├─ exec ─────────╢──► proceso especialista
║                             │         │                    └─ http ─────────╢──► servicio especialista
║                             ▼         ▼                                     ║
║                       backends System One (Jev vía OpenRouter, TypeSafe,    ║
║                       Ollama…) — custodia de claves, plazos, coste          ║
║                                                                             ║
║   bus de eventos ──── WebSocket /events ────► TUI (dashboard en vivo)       ║
╚═════════════════════════════════════════════════════════════════════════════╝
                                  ▲
                                  │ mismo camino, en proceso, sin HTTP
                         jevalan lab (offline / record / live)
```

Tres principios atraviesan todo:

1. **Jev juzga, el código actúa.** Un modelo System One devuelve probabilidades tipadas; reglas y umbrales locales convierten esas probabilidades en una decisión; el agente aplica el efecto.
2. **El agente no sabe quién decide.** Pide una capacidad (`tool.risk`, `agent.wakeup`); que la sirva un flujo propio o un especialista ajeno se elige en la configuración.
3. **La telemetría es explícita.** Los eventos no tienen un campo para la entrada original, pero pueden llevar respuestas, outcome, señales, definiciones y errores; trata el WebSocket como datos sensibles.

## 2. Un proveedor `exec` ajeno: Toolgate (ejemplo de integración)

Este ejemplo describe `config/capabilities/tool.risk.yaml`, que lanza Toolgate y puede enviar entradas a servicios remotos. **No es la configuración del hook recomendada**: Claude Code y Crush deben usar `config/lab/config.local.yaml` y el flujo local `tool-risk-local`; no apuntes el hook a Toolgate o a modelos en la nube mientras no exista redacción de salida.
Toolgate es un cortafuegos de herramientas para agentes. En jevalan actúa como **especialista**: jevalan lo lanza como proceso, le pasa la entrada y clasifica su respuesta. El comando sale de la configuración, con `${VAR}` resuelto al cargar la capacidad (entorno o `.env`): `node ${TOOLGATE_CLI} decide`.

```
 TU AGENTE (Claude Code, Crush, un script…)
   │  POST /v1/hub  { capability: "tool.risk", input: {tool_name: "Bash", command: "git status"} }
   ▼
╔══════════════════════════ JEVALAN (nuestro hub) ═══════════════════════════╗
║  1. envoltura jev-hub/0.1 ok?          4. plazo = min(pedido, 5000 ms)    ║
║  2. capacidad "tool.risk" v1 existe?   5. ¿qué proveedor la sirve?        ║
║  3. input cumple su JSON Schema?          → config/capabilities/tool.risk.yaml   ║
║                                            provider: exec "toolgate"      ║
║      ┌────────────────────────────────────────────────┐                   ║
║      │ lanza un proceso SIN shell, entorno limpio      │                   ║
║      │ (solo PATH + las claves de la lista `env`)      │                   ║
║      │ escribe el input por stdin ─────────────────┐  │                   ║
║      └─────────────────────────────────────────────│──┘                   ║
╚════════════════════════════════════════════════════│═══════════════════════╝
                                                     ▼
      ┌──────────────────────── TOOLGATE (proceso node, especialista ajeno) ─┐
      │  a) reglas fijas (regex)  ── rm -rf / → DENY   curl|sh → ASK  ──┐     │
      │        │ no casa ninguna                                        │     │
      │        ▼                                                        │     │
      │  b) una llamada a Jev con 7 preguntas (sí/no) ─────────────────────────────►  OPENROUTER
      │       destruye? exfiltra? privilegios? secretos?                │     │       Decisions API
      │       fuera de tarea? viola restricción? decisión reservada?   ◄──────────────  Jev: p de cada una
      │       + ¿autorizado por la tarea?                               │     │
      │        ▼                                                        │     │
      │  c) umbrales: riesgo máx ≥ 0.85 → deny · ≥ 0.55 → ask · resto → allow │
      │     (suaviza si está autorizado, nunca los secretos)            │     │
      │        ▼                                                        ▼     │
      │  stdout: {verdict, reason, source, probabilities}                     │
      └──────────────────────────────────────┬────────────────────────────────┘
                                             ▼
╔════════════════════════════════ JEVALAN ═══════════════════════════════════╗
║  6. ¿source = no-opinion? → ABSTAIN   ¿source = fail-mode? → ERROR       ║
║  7. la decisión cumple su JSON Schema? si no → ERROR invalid_decision    ║
║  8. HUB_REQUEST / HUB_RESPONSE ──────────────────────────► TUI en vivo ║
║     Los eventos pueden contener respuestas, config y errores.           ║
╚═══════════════════════════════════════════╤═════════════════════════════════╝
                                            ▼
 TU AGENTE ◄─ { status: "ok", decision: {verdict: "allow", …}, provider: "toolgate", trace }
             y él decide qué hacer: ejecutar, preguntarte o no hacerlo
```

Garantías del lado de jevalan: el proceso nunca pasa por una shell, no hereda el entorno del daemon, se mata con todo su árbol si agota el plazo, su salida se limita a 1 MiB, y `exec` está desactivado salvo `hub.allow_exec: true`. `abstain` y `error` nunca llevan `decision`.

## 3. Una capacidad servida por un flujo propio

### 3.1 `agent.wakeup` (hoy)

Reimplementa Wakegate dentro de jevalan: tres reglas y una sola pregunta a Jev.

```
 TU AGENTE ─► JEVALAN ─ capacidad agent.wakeup ─ provider: flow "agent-wakeup"
                 │
                 ├─ rule  "input.from_user"                         ─ true ─► despierta (user-message)
                 ├─ rule  "!has(input.event) && !has(input.observation)" ─ true ─► despierta (nothing-to-judge)
                 ├─ rule  "input.skipped >= max_skips (10)"         ─ true ─► despierta (skip-limit)
                 ├─ noul  "¿el evento justifica despertarlo?" ─────────────────────►  OPENROUTER / Jev
                 │                                                  ◄────────────  p
                 ├─ p < 0.3        ─► duerme (judged)
                 ├─ 0.3 ≤ p < 0.5  ─► despierta (unsure)
                 ├─ p ≥ 0.5        ─► despierta (judged)
                 └─ error de Jev   ─► despierta (error) — ante la duda, despertar
```

El umbral de dormir pasó de 0.2 a 0.3 tras el primer ensayo real del laboratorio (2026-10-09): un evento ajeno caía justo en la frontera.

### 3.2 `tool.risk` como flujo nativo (implementado)

Toolgate es, por dentro, otro «decide»: reglas fijas, preguntas a Jev y umbrales. El flujo propio ya está escrito y es el que sirve `tool.risk` en local: `config/flows/tool-risk-local.yaml`, con la capacidad apuntándolo por `flow` en `config/capabilities-local/tool.risk.yaml`.

```
 TU AGENTE ─► JEVALAN ─ capacidad tool.risk ─ provider: flow "tool-risk-local"
                 │
                 ├─ rule  "rm -rf /"                          ─ true ─► deny
                 ├─ rule  "curl … | sh"                       ─ true ─► ask
                 ├─ batch de 4 preguntas en UNA llamada ──────────────►  KEV
                 │     destructive · exfiltration · privilege          ◄────────  p de cada una
                 │     secret_exposure
                 ├─ route "alguno ≥ 0.85"  ─ true ─► deny
                 ├─ route "alguno ≥ 0.55"  ─ true ─► ask
                 └──────────────────────────────────────────► allow
```

El nodo `batch` pide varias preguntas en una sola petición System One y reparte las respuestas con sus reglas `route`, evaluadas en orden; el CEL ve `answers.<nodo>.<pregunta>`. Antes eran hasta cuatro llamadas encadenadas —que se cortaban en el primer riesgo ≥ 0.55— y ahora es una sola, con lo que el umbral se aplica al conjunto y no al primero que aparezca.

## 4. Especialista ajeno frente a flujo propio

| | Especialista (`exec`/`http`) | Flujo propio (`flow`) |
|---|---|---|
| Quién tiene la receta | el especialista (Toolgate) | nosotros (un YAML en `config/flows/`) |
| Calibración | la suya, evaluada por sus autores | la nuestra, medida con el laboratorio |
| Dependencias | el runtime del especialista (Node para Toolgate) | ninguna |
| Visibilidad en la TUI | un chip con el especialista | el grafo nodo a nodo |
| Coste por decisión | lo que haga el especialista (Toolgate: 1 llamada con 7 preguntas) | una llamada por nodo Jev; con un nodo `batch`, **una** para varias preguntas |
| Lo que trae de serie | contexto de la tarea, redacción de secretos, libro de acciones, auditoría | lo que escribamos |

Las dos formas son intercambiables para el agente: la capacidad se pide igual y se cambia de proveedor en `config/capabilities/<nombre>.yaml`. El laboratorio permite pasar los mismos fixtures a ambas y comparar si deciden igual de bien.

## 5. El laboratorio

```
 config/lab/fixtures/<capacidad>/<caso>.yaml          config/lab/lab.yaml (targets, presupuesto, repeticiones)
 (entrada + aserciones CEL con su `why`)                │
          │                                             │
          ▼                                             ▼
 ┌─────────────────────────── jevalan lab ───────────────────────────┐
 │  offline: respuestas grabadas (replay a mano o cassette), sin red │
 │  record:  llamadas reales que se graban como cassettes            │
 │  live:    llamadas reales, fixtures × targets × repeticiones      │
 │                   │                                               │
 │                   ▼                                               │
 │   hub::execute(..., Transport::{Replay | Record | Live})          │
 │   — el MISMO camino que POST /v1/hub, en proceso                  │
 └──────────────────────────────┬────────────────────────────────────┘
                                ▼
 .local/runs/<id>/ manifest.json · results.jsonl · report.md
 (pasa, estable, abstain/error, p50/p95, coste real, calibración Brier)
```

`offline` es la regresión de nuestro código (determinista, en CI); `live` mide la calidad real de un backend o un especialista con un presupuesto declarado.

## 6. Claude Code como cliente: el hook `PreToolUse`

Claude Code avisa a jevalan **antes** de cada herramienta. El hook es un subcomando del propio binario, `jevalan hook claude-code`, instalado en la configuración global de Claude Code; pide la capacidad `tool.risk` al daemon y, según el modo, calla o responde.

```
 CLAUDE CODE (cualquier sesión)
   │  va a usar una herramienta → hook PreToolUse
   │  stdin: {tool_name, tool_input, cwd, session_id, tool_use_id, transcript_path}
   ▼
 jevalan hook claude-code [--mode shadow|enforce]        (proceso corto, sin estado)
   │  ¿hay daemon? → fichero de descubrimiento que deja `serve` al arrancar
   │     no hay  ─► sombra: calla y sale 0      activo: responde ASK
   │
   │  POST /v1/hub { capability: "tool.risk", input: {…, transcript_path} }
   ▼
╔══════════════════ JEVALAN (daemon) ══════════════════╗
║  tool.risk ─► tool-risk-local ─► Kev/Laya locales       ║──► eventos ──► TUI
║  flow local: cuatro preguntas de riesgo en una llamada  ║
╚════════════════════════╤══════════════════════════════╝
                         ▼
 jevalan hook claude-code
   │  apunta la decisión en el registro sombra (local, secretos con forma conocida tachados)
   │  ──► más adelante: de ahí salen fixtures reales para el laboratorio
   │
   ├─ modo SOMBRA (por defecto) ─► no responde nada: Claude sigue exactamente igual
   └─ modo ACTIVO               ─► deny  → bloquea con el motivo de la capacidad
                                   ask   → Claude te pregunta
                                   allow → NO responde (nunca se salta tus permisos)
                                   error/abstain/plazo → ASK, jamás un allow a ciegas
```

La TUI (`jevalan tui`, tecla `H`) lee `shadow.jsonl` en local. El evento no transporta un campo `input`, pero sus respuestas (`NodeEnd.answer`), outcomes (`DecisionMade.outcome`), señales (`NodeAudit.signals`), definiciones (`FlowLoaded.definition`) y errores pueden contener datos sensibles o reproducir contenido. No compartas capturas del bus sin revisarlas.

El formato del evento es v2 (`src/crates/jevalan-events/src/v2.rs`). `manifest_version: "2"` identifica el formato del manifiesto; `version: "1"` en una capacidad es su versión de contrato.

El hook solo puede **endurecer** los permisos de Claude Code, nunca relajarlos: en el peor caso pregunta de más.
