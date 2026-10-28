---
title: HTTP API
description: Every route of the local daemon, with requests and real responses.
---

The daemon listens on `127.0.0.1:8080` by default (`server.addr`). All bodies are JSON.

## Authentication

If the variable named by `server.token_env` (`JEVALAN_TOKEN` by default) is set, every route except `/healthz` requires a bearer token:

```sh
curl -s http://127.0.0.1:8080/v1/flows -H "Authorization: Bearer $JEVALAN_TOKEN"
```

The `/events` WebSocket also accepts `?token=<token>`, because browsers cannot set headers on a WebSocket.

## Routes

| Route | What it does |
|---|---|
| `GET /healthz` | Returns `ok`. No authentication. |
| `POST /{backend}/v1/systemone` | Forwards the body, byte for byte, to that backend. |
| `POST /v1/systemone` | The same, against `default_backend`. |
| `POST /v1/decide/{flow}` | Runs a named flow. |
| `GET /v1/flows` | Lists loaded flows and the invalid ones, with the reason. |
| `POST /v1/flows/validate` | Validates a flow without publishing it. |
| `POST /v1/flows/draft/run` | Runs a flow text once without publishing it. |
| `POST /v1/hub` | Asks a [capability](/en/capacidades/) for a typed decision. |
| `GET /v1/hub/capabilities` | Lists loaded capabilities and the invalid ones. |
| `GET /events` | WebSocket with one JSON event per message. |

## Run a flow

`state` can be any JSON value; your CEL expressions read it as `input`. `client` is optional and only labels the request in the TUI.

```sh
curl -s http://127.0.0.1:8080/v1/decide/ticket-triage \
  -H 'Content-Type: application/json' \
  -d '{"state":{"id":"T-43","priority":"low","topic":"billing"},"client":"docs"}'
```

```json
{"data":{"team":"finance","ticket":"T-43"},"flow":"ticket-triage","outcome":"ok","path":[{"answer":false,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"urgent","port":"false"},{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"billing","port":"true"}],"trace_id":"18e2ca4a68e5b5b40003"}
```

| Field | Meaning |
|---|---|
| `outcome` | The `outcome` of the output the flow reached. |
| `data` | The output's `data`, with references already resolved. |
| `path` | Every node visited: its `port`, the model's `answer` and `confidence`, and `duration_ms`. |
| `trace_id` | Identifier you can follow in the TUI. |

Errors are JSON with an `error` message:

| Status | When |
|---|---|
| `400` | The body has no `state`: `{"error":"falta «state»"}`. |
| `404` | Unknown flow: `{"error":"flujo desconocido: nope"}`. |
| `502` | A node failed and has no `error` port, or the backend is unreachable. |

## Validate and try a draft

Both routes take the flow name and its YAML as text, and neither publishes anything. Use them from an editor, a CI job, or a script.

```sh
curl -s http://127.0.0.1:8080/v1/flows/validate \
  -H 'Content-Type: application/json' \
  -d '{"flow":"x","yaml":"description: t\ndefaults: { backend: ollama, model: nimble }\nstart: a\nnodes:\n  a:\n    type: rule\n    when: \"input.x ==\"\n    out: { \"true\": y, \"false\": z }\noutputs:\n  y: { outcome: ok }\n"}'
```

An invalid flow answers `ok: false` with the exact problem, including the position inside the CEL expression:

```json
{"error":"a: expresión CEL inválida: ERROR: <input>:1:11: Syntax error: mismatched input '<EOF>' …","ok":false}
```

A valid one answers `{"ok": true, "flow": {…}}`. To also execute a draft, add an `input` and a `transport`:

```sh
curl -s http://127.0.0.1:8080/v1/flows/draft/run \
  -H 'Content-Type: application/json' \
  -d '{"flow":"x","yaml":"…","input":{"x":1},"transport":"live"}'
```

```json
{"confidence":null,"data":{"r":"yes"},"flow":"x","outcome":"ok","path":[{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"a","port":"true"}],"trace_id":"18e2ca4a96f0d9840007"}
```

## Proxy to a backend

The passthrough forwards your System One request untouched and adds the backend's API key if one is configured. Identify your client with the `X-Jevalan-Client` header; the default is `unknown`.

```sh
curl -s http://127.0.0.1:8080/ollama/v1/systemone \
  -H 'Content-Type: application/json' \
  -H 'X-Jevalan-Client: my-agent' \
  -d '{"model":"nimble","state":"ls -la","questions":{"q":{"type":"noul","instructions":"Is it destructive?"}}}'
```

If the backend is unreachable the daemon answers `502`.

## Capabilities

See [Capabilities and hub](/en/capacidades/) for `POST /v1/hub` and `GET /v1/hub/capabilities`.

## Events

`GET /events` upgrades to a WebSocket and publishes one JSON event per message: requests received, nodes started and finished, decisions made, flows and capabilities loaded, and errors. The terminal UI is a client of this stream.

Events carry no field with the original input, but they can carry answers, outcomes, signals, flow definitions, and errors. Treat the stream as sensitive data, and do not share a capture without reviewing it.
