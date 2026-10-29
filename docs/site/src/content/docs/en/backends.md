---
title: Compatible backends
description: What a server must meet to serve as a Jevalan backend, how to check it with three requests, and what happens when it does not.
---

Any service that speaks the [System One](/en/system-one/) contract can be a Jevalan backend: a hosted one, a local server over an open model, or a simulator. Jevalan does not tell them apart. This page says what is required of one, how to check it before plugging it in, and what happens when it fails.

## Registering a backend

A backend is an entry under `backends` in the [configuration](/en/configuracion/):

```yaml
backends:
  local: { url: "http://127.0.0.1:9000", timeout_ms: 5000 }
default_backend: local
```

Flows name it with `backend: local`; the rest of the configuration (key, path) is in [Configuration](/en/configuracion/).

## What is required

A [flow](/en/flujos/) reads the response with strict rules. Any violation leaves through the node's `error` port, not as a failed request:

| Requirement | If it is not met |
|---|---|
| `answers.<question>` exists, with the same name that was sent. | `la respuesta no trae la pregunta <name>`. |
| If the answer carries `type`, it matches the node's. | `respuesta de tipo <x>, se esperaba <y>`. |
| It carries the field named after the type: `noul`, `choice` or `score`. | `falta el campo <type>`. |
| `choice` and `score` carry `confidence`. For `noul` it is optional: if absent, Jevalan computes it from the probability. | `falta confidence`. |
| `confidence` is between 0 and 1. | `confianza fuera de [0, 1]`. |
| The `noul` value is a number. | `noul debe ser un número`. |

The server must also accept **several questions in a single request** if you use `batch` nodes (a `batch` makes one call with all of them), and the **three types** (`noul`, `choice`, `score`) if your flows use them. A server that implements only some types is not defective, but every node of an unsupported type will leave through `error`: see example 4 in [Examples](/en/ejemplos/).

## Checking it with three requests

Run them **against the server itself**, not through Jevalan, to know whose problem it is. Change the URL and `<model-id>`:

```sh
URL=http://127.0.0.1:9000/v1/systemone

# noul: returns a probability of yes
curl -s $URL -H 'Content-Type: application/json' -d '{"model":"<model-id>","state":"rm -rf /","questions":{"q":{"type":"noul","instructions":"Is the command destructive?"}}}'

# choice: returns one of the keys, with probabilities and confidence
curl -s $URL -H 'Content-Type: application/json' -d '{"model":"<model-id>","state":"The app crashes on export.","questions":{"q":{"type":"choice","instructions":"What does the user want?","criteria":{"bug":"Something is broken","howto":"A usage question","other":"Anything else"}}}}'

# score: returns a position on the scale, with confidence
curl -s $URL -H 'Content-Type: application/json' -d '{"model":"<model-id>","state":"The app crashes on export.","questions":{"q":{"type":"score","instructions":"How urgent is it?","criteria":["none","low","medium","high"]}}}'
```

Each response should look like those in [the reference](/en/system-one/): an `answers.q` with its `type` and the type's field. For the several-questions case, repeat the first with two differently named questions and check that both come back.

When all three work, repeat them against Jevalan (`POST /local/v1/systemone`, see [HTTP API](/en/api/)) to check the key and the deadline, and run a one-node flow to see its `path`. If the node leaves through `error`, the reason is in the [trace](/en/solucion-de-problemas/).

## When something does not fit

| Symptom | Likely cause |
|---|---|
| Every node of one type leaves through `error`. | The server does not implement that type. |
| `falta confidence` on `choice` or `score`. | The server returns probabilities but not the `confidence` field. |
| The `batch` fails and single nodes do not. | The server accepts only one question per request. |
| `502` or timeouts. | Wrong `url`, `path` or `timeout_ms`, or the server is slow with your `state`. |
| Decisions change between runs. | Normal for a model: measure it with repetitions in the [lab](/en/laboratorio/). |

A different server does not produce the same numbers. A flow's thresholds are tuned to a backend: when you change backend, measure them again with the lab (`jevalan lab live`) before switching to `enforce`.
