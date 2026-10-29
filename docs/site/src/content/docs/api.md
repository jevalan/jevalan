---
title: API HTTP
description: Todas las rutas del daemon local, con peticiones y respuestas reales.
---

El daemon escucha por defecto en `127.0.0.1:8080` (`server.addr`). Todos los cuerpos son JSON.

## Autenticación

Si la variable que nombra `server.token_env` (`JEVALAN_TOKEN` por defecto) está definida, todas las rutas salvo `/healthz` exigen un token bearer:

```sh
curl -s http://127.0.0.1:8080/v1/flows -H "Authorization: Bearer $JEVALAN_TOKEN"
```

El WebSocket de `/events` también acepta `?token=<token>`, porque los navegadores no pueden poner cabeceras en un WebSocket.

Sin un token válido, una ruta responde `401` con el texto plano `token requerido` (no JSON), y `/events` rechaza la actualización con el mismo estado.

## Rutas

| Ruta | Qué hace |
|---|---|
| `GET /healthz` | Devuelve `ok`. Sin autenticación. |
| `POST /{backend}/v1/systemone` | Reenvía el cuerpo, byte a byte, a ese backend. |
| `POST /v1/systemone` | Lo mismo, contra `default_backend`. |
| `POST /v1/decide/{flujo}` | Ejecuta un flujo por nombre. |
| `GET /v1/flows` | Lista los flujos cargados y los inválidos, con el motivo. |
| `POST /v1/flows/validate` | Valida un flujo sin publicarlo. |
| `POST /v1/flows/draft/run` | Ejecuta el texto de un flujo una vez sin publicarlo. |
| `POST /v1/hub` | Pide una decisión tipada a una [capacidad](/capacidades/). |
| `GET /v1/hub/capabilities` | Lista las capacidades cargadas y las inválidas. |
| `GET /events` | WebSocket con un evento JSON por mensaje. |

## Ejecutar un flujo

`state` puede ser cualquier valor JSON; tus expresiones CEL lo leen como `input`. `client` es opcional y solo etiqueta la petición en la TUI.

```sh
curl -s http://127.0.0.1:8080/v1/decide/ticket-triage \
  -H 'Content-Type: application/json' \
  -d '{"state":{"id":"T-43","priority":"low","topic":"billing"},"client":"docs"}'
```

```json
{"data":{"team":"finance","ticket":"T-43"},"flow":"ticket-triage","outcome":"ok","path":[{"answer":false,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"urgent","port":"false"},{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"billing","port":"true"}],"trace_id":"18e2ca4a68e5b5b40003"}
```

| Campo | Significado |
|---|---|
| `outcome` | El `outcome` de la salida a la que llegó el flujo. |
| `data` | El `data` de la salida, con las referencias ya resueltas. |
| `path` | Cada nodo visitado: su `port`, la `answer` y la `confidence` del modelo, y `duration_ms`. |
| `trace_id` | Identificador que puedes seguir en la TUI. |

Los errores son JSON con un mensaje `error`:

| Estado | Cuándo |
|---|---|
| `400` | El cuerpo no tiene `state`: `{"error":"falta «state»"}`. |
| `404` | Flujo desconocido: `{"error":"flujo desconocido: nope"}`. |
| `401` | Falta el token o es incorrecto. El cuerpo es el texto plano `token requerido`. |
| `502` | Un nodo falló y no tiene puerto `error`, o el backend no responde. |

## Validar y probar un borrador

Las dos rutas reciben el nombre del flujo y su YAML como texto, y ninguna publica nada. Úsalas desde un editor, un trabajo de CI o un script.

```sh
curl -s http://127.0.0.1:8080/v1/flows/validate \
  -H 'Content-Type: application/json' \
  -d '{"flow":"x","yaml":"description: t\ndefaults: { backend: ollama, model: nimble }\nstart: a\nnodes:\n  a:\n    type: rule\n    when: \"input.x ==\"\n    out: { \"true\": y, \"false\": z }\noutputs:\n  y: { outcome: ok }\n"}'
```

Un flujo inválido responde `ok: false` con el problema exacto, incluida la posición dentro de la expresión CEL:

```json
{"error":"a: expresión CEL inválida: ERROR: <input>:1:11: Syntax error: mismatched input '<EOF>' …","ok":false}
```

Uno válido responde `{"ok": true, "flow": {…}}`. Para ejecutar además un borrador, añade un `input` y un `transport`:

```sh
curl -s http://127.0.0.1:8080/v1/flows/draft/run \
  -H 'Content-Type: application/json' \
  -d '{"flow":"x","yaml":"…","input":{"x":1},"transport":"live"}'
```

```json
{"confidence":null,"data":{"r":"yes"},"flow":"x","outcome":"ok","path":[{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"a","port":"true"}],"trace_id":"18e2ca4a96f0d9840007"}
```

`transport` es `live` (llama a los backends reales), `record` (también los llama y devuelve en la respuesta el `cassette` grabado) o `replay` (toma un `cassette` de la petición y no llama a nadie). Cualquier otro valor es un `400`.

## Proxy a un backend

El passthrough reenvía tu petición System One sin tocarla y añade la clave del backend si hay una configurada. Identifica a tu cliente con la cabecera `X-Jevalan-Client`; por defecto es `unknown`.

```sh
curl -s http://127.0.0.1:8080/ollama/v1/systemone \
  -H 'Content-Type: application/json' \
  -H 'X-Jevalan-Client: mi-agente' \
  -d '{"model":"nimble","state":"ls -la","questions":{"q":{"type":"noul","instructions":"¿Es destructivo?"}}}'
```

Si el backend no responde, el daemon contesta `502`.

## Capacidades

Mira [Capacidades y hub](/capacidades/) para `POST /v1/hub` y `GET /v1/hub/capabilities`.

## Eventos

`GET /events` se convierte en un WebSocket y publica un evento JSON por mensaje: peticiones recibidas, nodos iniciados y terminados, decisiones tomadas, flujos y capacidades cargados, y errores. La interfaz de terminal es un cliente de este flujo. Al conectarte recibes primero el estado actual (los backends y los flujos cargados) y después los eventos en vivo.

Los eventos no llevan un campo con la entrada original, pero sí pueden llevar respuestas, resultados, señales, definiciones de flujo y errores. Trata el flujo como datos sensibles y no compartas una captura sin revisarla.
