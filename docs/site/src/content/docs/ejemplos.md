---
title: Ejemplos
description: Cinco ejemplos completos, desde un flujo solo con reglas hasta un guardián para tu agente de código y una comprobación en CI.
---

Cada ejemplo es autosuficiente. Los dos primeros se ejecutaron contra un daemon real y las respuestas son su salida auténtica; el resto usa ficheros y comandos que vienen con el repositorio.

## 1. Enruta tickets de soporte con reglas

Sin modelo y sin llamar a ningún backend: el flujo solo lee la petición y sigue puertos. Guárdalo como `config/flows/ticket-triage.yaml`:

```yaml
description: "Route a support ticket"
defaults: { backend: ollama, model: nimble }
start: urgent
nodes:
  urgent:
    type: rule
    when: 'input.priority == "high"'
    out: { "true": page, "false": billing }
  billing:
    type: rule
    when: 'input.topic == "billing"'
    out: { "true": finance, "false": queue }
outputs:
  page:    { outcome: ok, data: { team: oncall,  ticket: "$input/id" } }
  finance: { outcome: ok, data: { team: finance, ticket: "$input/id" } }
  queue:   { outcome: ok, data: { team: support, ticket: "$input/id" } }
```

```sh
curl -s http://127.0.0.1:8080/v1/decide/ticket-triage \
  -H 'Content-Type: application/json' \
  -d '{"state":{"id":"T-43","priority":"low","topic":"billing"}}'
```

La respuesta es `outcome: ok` con `data: {"team":"finance","ticket":"T-43"}`, más el `path` que llevó hasta ahí. En [Flujos](/flujos/) tienes todos los tipos de nodo.

## 2. Pon una barrera a un despliegue

La misma idea, usada como comprobación de seguridad antes de un paso del pipeline. Dos reglas deciden y el flujo devuelve `allow`, `ask` o `deny` con un motivo:

```yaml
description: "Gate a deployment with plain rules"
defaults: { backend: ollama, model: nimble }
start: prod_from_branch
nodes:
  prod_from_branch:
    type: rule
    when: 'input.env == "prod" && input.branch != "main"'
    out: { "true": deny_branch, "false": tests }
  tests:
    type: rule
    when: '!input.tests_passed'
    out: { "true": ask_tests, "false": allow }
outputs:
  deny_branch: { outcome: deny, data: { reason: "production deploys must come from main" } }
  ask_tests:   { outcome: ask,  data: { reason: "tests have not passed", branch: "$input/branch" } }
  allow:       { outcome: allow }
```

| `state` de la petición | `outcome` | `data` |
|---|---|---|
| `{"env":"prod","branch":"feature/x","tests_passed":true}` | `deny` | `{"reason":"production deploys must come from main"}` |
| `{"env":"prod","branch":"main","tests_passed":false}` | `ask` | `{"branch":"main","reason":"tests have not passed"}` |
| `{"env":"prod","branch":"main","tests_passed":true}` | `allow` | ninguno |

Tu pipeline lee `outcome` y decide qué hacer; Jevalan nunca ejecuta el despliegue.

## 3. Vigila a tu agente de código

Pon primero el hook en modo `shadow`. Registra lo que Jevalan habría decidido y nunca interfiere:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "*", "hooks": [ { "type": "command", "command": "/path/to/jevalan hook claude-code --mode shadow" } ] }
    ]
  }
}
```

Trabaja con normalidad unos días y luego lee lo que vio:

```sh
jevalan shadow report --since 2026-10-01 --out report.md
```

Cuando los veredictos coincidan con tu propio criterio, cambia `--mode shadow` por `--mode enforce`. En `enforce` el hook solo puede endurecer permisos, y cuando no puede decidir, pregunta. Mira [Integraciones](/integraciones/).

## 4. Elige modelo según la dificultad

`config/flows/model-route.yaml` puntúa una tarea y manda las fáciles a un modelo pequeño y las difíciles a uno grande, con una pregunta de sí o no para todo lo que toque seguridad, datos de producción o dinero:

```yaml
start: tier
nodes:
  tier:
    type: score
    instructions: "¿Cuánta dificultad tiene la tarea?"
    criteria: ["trivial", "sencilla", "media", "difícil", "muy difícil"]
    out: { "0..1.5": small, "1.5..3": risky, "3..4": large, error: medium, low-confidence: medium }
  risky:
    type: noul
    instructions: "¿La tarea toca seguridad, datos de producción o dinero?"
    out: { "true": large, "false": medium }
outputs:
  small:  { outcome: route, data: { model: haiku,  effort: low } }
  medium: { outcome: route, data: { model: sonnet, effort: medium } }
  large:  { outcome: route, data: { model: opus,   effort: high } }
```

A diferencia de los dos primeros, este flujo pregunta a un modelo, así que necesita un backend alcanzable. Fíjate en los puertos `error` y `low-confidence`: si el modelo falla o duda, el flujo cae en la opción intermedia en lugar de hacer fallar la petición.

## 5. Comprueba las decisiones en CI

Añade el laboratorio a tu pipeline. No necesita backend ni clave:

```sh
jevalan lab check && jevalan lab offline
```

`lab check` valida los fixtures (imprime `lab en orden: 16 fixtures` en este repositorio). `lab offline` los reproduce e imprime una tabla por capacidad con las columnas `pasa`, `skipped` y `fallan`; sale con error si algún caso falla.

Para validar un flujo antes de que llegue al daemon, envíalo a `POST /v1/flows/validate`. Un flujo inválido responde `ok: false` con el problema exacto, incluida la posición dentro de una expresión CEL. Mira [API](/api/) y [Laboratorio](/laboratorio/).
