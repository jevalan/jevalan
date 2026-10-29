---
title: System One reference
description: The System One contract, the typed-decision API that Jevalan forwards and its flows use, with a downloadable OpenAPI file.
---

System One is a fast, typed decision API: you send a `state` and up to 64 named questions, and it returns a typed answer, with a confidence, for each. Jevalan forwards it as is (passthrough), and it is what the `noul`, `choice` and `score` nodes of a [flow](/en/flujos/) speak underneath.

:::caution[Not the official specification]
This page describes the contract **as Jevalan observes it**: its code, its test server and the public documentation of gateways that expose the same endpoint. What those sources do not fix (the error body, size limits) is not invented here. When in doubt, the documentation of your provider rules.
:::

Download the OpenAPI 3.1 description of this contract: [`system-one.openapi.yaml`](/system-one.openapi.yaml). Use it to generate a client, validate requests or open it in any OpenAPI viewer.

## Endpoint

`POST /v1/systemone`, JSON body and response, with `Authorization: Bearer <key>`. Through Jevalan:

| Route | Where it goes |
|---|---|
| `POST /v1/systemone` | The default backend (`default_backend`). |
| `POST /{backend}/v1/systemone` | The backend you name. |

Jevalan forwards the body byte for byte, adds the backend's key if one is configured and returns the backend's status and body untouched. If the backend does not answer, it replies `502`. Details in [HTTP API](/en/api/) and [Configuration](/en/configuracion/).

## The request

| Field | Type | Meaning |
|---|---|---|
| `model` | text | The model identifier on that backend. |
| `state` | text | What is being judged. |
| `questions` | object | Named questions: from 1 to 64. |

Each question carries a `type` and, depending on the type:

| `type` | `instructions` | `criteria` | Answers |
|---|---|---|---|
| `noul` | The question, phrased for yes or no. | Optional: only the keys `"true"` and `"false"`, with what each means. | A probability of yes. |
| `choice` | The question. | Required: map of key → description for each option. | One of the keys. |
| `score` | The question. | Required: list of levels, lowest first. | A position on that scale. |

```json
{
  "model": "<model-id>",
  "state": "I was charged twice for my subscription.",
  "questions": {
    "refund": { "type": "noul", "instructions": "Is the customer asking for money back?" },
    "department": {
      "type": "choice",
      "instructions": "Which team should handle this?",
      "criteria": { "billing": "Charges and refunds", "technical": "Bugs and outages" }
    },
    "urgency": { "type": "score", "instructions": "How urgent is it?", "criteria": ["none", "low", "medium", "high"] }
  }
}
```

## The response

One entry in `answers` per question, with the same name, plus the call's `usage`:

| Type | Response fields |
|---|---|
| `noul` | `noul` (probability of yes, 0 to 1) and `confidence`. |
| `choice` | `choice` (the chosen key), `probabilities` (one per key) and `confidence`. |
| `score` | `score` (position on the scale, from 0 to the last level's index) and `confidence`. |

```json
{
  "model": "<model-id>",
  "answers": {
    "refund": { "type": "noul", "noul": 0.98, "confidence": 0.9 },
    "department": { "type": "choice", "choice": "billing", "probabilities": { "billing": 0.91, "technical": 0.09 }, "confidence": 0.88 },
    "urgency": { "type": "score", "score": 2.1, "confidence": 0.74 }
  },
  "usage": { "input_tokens": 275, "output_tokens": 20 }
}
```

The numbers in the example are samples. The response is not streamed: it arrives whole.

## How a flow uses it

A [flow](/en/flujos/) node is a System One question with output ports:

| Node | Question | Output ports |
|---|---|---|
| `noul` | `type: noul` | `"true"` / `"false"` according to the `threshold`. |
| `choice` | `type: choice`, with `criteria` | One per key in `criteria`. |
| `score` | `type: score`, with `criteria` | `a..b` ranges over the scale. |
| `batch` | Several questions in a single call | The `route` rules see every answer. |

Nodes that ask a model can also wire the `error` port (the backend failed or answered badly) and the `low-confidence` port (the `confidence` fell below `min_confidence`; if you do not wire it, the confidence is ignored, and `batch` does not accept it). That is why a backend that does not support a question type sends every request out the `error` port: see example 4 in [Examples](/en/ejemplos/).

## Errors

The public sources fix only a few cases: a `noul` question with neither `instructions` nor `criteria` answers `400`, a wrong key `401`, and an access denied by a gateway `403`. There is no common schema for the error body, so treat any status other than `200` as "no answer" and choose the default as the [decision guide](/en/guia-decisiones/) explains.
