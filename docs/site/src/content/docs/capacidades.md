---
title: Capacidades y hub
description: Pide una decisión tipada por su nombre, sin saber quién la responde.
---

Una **capacidad** es una pregunta con nombre y tipos, como `tool.risk` («¿es arriesgada esta llamada a una herramienta?») o `agent.wakeup` («¿merece la pena despertar a este agente?»). Tu agente le pide la capacidad al hub; qué flujo, proceso o servicio la responde se decide en la configuración. Puedes cambiar el proveedor sin tocar el agente.

## Pedir una decisión

```sh
curl -s http://127.0.0.1:8080/v1/hub \
  -H 'Content-Type: application/json' \
  -d '{"protocol_version":"jev-hub/0.1","request_id":"r1","capability":"agent.wakeup","capability_version":"1","input":{"waiting_for":"CI to finish","from_user":true,"skipped":0}}'
```

```json
{"capability":"agent.wakeup","capability_version":"1","decision":{"reason":"user-message","wake":true},"protocol_version":"jev-hub/0.1","provider":{"id":"jevalan-flow","version":"0.1.0"},"request_id":"r1","status":"ok","trace":{"cost":"not_measured","elapsed_ms":2,"path":[{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"user","port":"true"}],"trace_id":"18e2ca4a7cd8c9d00005"}}
```

| Campo de la petición | Significado |
|---|---|
| `protocol_version` | Siempre `jev-hub/0.1`. |
| `request_id` | Tu identificador; vuelve en la respuesta. |
| `capability`, `capability_version` | Qué pregunta, y qué versión de ella. |
| `input` | Los datos a juzgar. Debe cumplir el `input_schema` de la capacidad. |
| `constraints.deadline_ms` | Opcional. Solo puede acortar el plazo por defecto de la capacidad. |

## La respuesta

La respuesta siempre tiene un `status`:

| Estado | Significado | Lleva `decision` |
|---|---|---|
| `ok` | El proveedor decidió. | Sí, válida contra el `decision_schema`. |
| `abstain` | El proveedor prefirió no opinar. | No |
| `error` | Algo falló, con un `error.code` estable. | No |

Tu agente debería tratar `abstain` y `error` como «sin respuesta» y elegir un valor seguro por defecto; nunca llevan `decision`. Por ejemplo, una capacidad desconocida:

```json
{"capability":"nope.nope","capability_version":"1","error":{"code":"unknown_capability","message":"capacidad desconocida: nope.nope","retryable":false},"protocol_version":"jev-hub/0.1","provider":{"id":"-","version":"-"},"request_id":"r2","status":"error","trace":{"cost":"not_measured","elapsed_ms":0,"trace_id":"18e2ca4a825001080006"}}
```

Otros códigos que puedes ver son `invalid_input` (el `input` no cumple el `input_schema`), `version_mismatch` (la `capability_version` no es la cargada), `provider_failed`, `deadline_exceeded`, `provider_disabled` e `invalid_decision`. `retryable` indica si volver a intentarlo puede servir.

El estado HTTP es `200` incluso con estos errores, porque viajan en `status`. Solo una petición mal formada, como un `protocol_version` distinto de `jev-hub/0.1`, responde `400`.

## Definir una capacidad

Una capacidad es un fichero YAML en `hub.capabilities_dir`. El nombre del fichero es el nombre de la capacidad (`agent.wakeup.yaml`):

```yaml
manifest_version: "2"
namespace: local.jev
version: "1"
description: "¿Merece la pena despertar a un agente dormido?"
input_schema:    { $file: "schemas/agent.wakeup.input.json" }     # JSON Schema, en línea o en un fichero
decision_schema: { $file: "schemas/agent.wakeup.decision.json" }
provider: { id: jevalan-flow, flow: agent-wakeup }                 # quién la responde
defaults: { deadline_ms: 3000 }
```

Se aplican los dos esquemas: una petición que no cumple el `input_schema` se rechaza, y una respuesta del proveedor que no cumple el `decision_schema` se convierte en `error`.

## Proveedores

| Proveedor | Clave | Quién responde |
|---|---|---|
| Flujo | `flow` | Un [flujo](/flujos/) tuyo, que corre dentro de Jevalan. |
| Http | `http` | Un POST a una `url`. |
| Exec | `exec` | Un proceso local: `command`, más los nombres de variables de entorno listados en `env`. |

`abstain_when` y `error_when` son expresiones opcionales sobre la `response` del proveedor que convierten una respuesta en `abstain` o `error`. Te dejan adaptar una herramienta de terceros sin modificarla. Si una expresión no se puede evaluar, por ejemplo porque lee un campo que la respuesta no tiene, la respuesta es un `error` con el código `provider_failed`.

### Lanzar procesos está desactivado por defecto

Los proveedores `exec` lanzan un programa en tu máquina, así que están desactivados salvo que pongas `hub.allow_exec: true` (o `JEVALAN_ALLOW_EXEC=1`). Cuando corren:

- El proceso se lanza **sin shell**, con un entorno vacío más `PATH` (y `SystemRoot` en Windows) y solo las variables nombradas en `env`.
- Se mata, con todo su árbol de procesos, si supera el plazo. La respuesta no se da por completa hasta que se cierra su salida: un proceso hijo que la herede y siga vivo retrasa la respuesta hasta el plazo, y si el proceso principal ya había terminado, ese hijo huérfano no se mata. Haz que tu especialista no deje procesos tras de sí.
- Su salida está limitada a 1 MiB.
- En Windows se rechazan los `.cmd` y `.bat`, porque lanzarlos pasa por una shell.
- El comando, su entorno y su URL nunca aparecen en eventos, registros ni en `GET /v1/hub/capabilities`.

Con `allow_exec` desactivado, una capacidad `exec` sigue siendo válida pero responde `provider_disabled`.

## Ver lo que hay cargado

```sh
curl -s http://127.0.0.1:8080/v1/hub/capabilities
```

La respuesta lista cada capacidad con su versión, descripción, tipo de proveedor, esquemas y plazo por defecto, más un objeto `invalid` que explica las que no se pudieron cargar. El directorio se vigila: guardar un fichero válido sustituye la capacidad, uno inválido conserva la versión anterior, y una capacidad de tipo `flow` pasa a inválida mientras falte su flujo.
