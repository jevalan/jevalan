---
title: Flows
description: Write decision flows in YAML with rules, model questions, and typed outputs.
---

A **flow** is a small decision graph written in YAML. It starts at one node, follows ports from node to node, and ends at an **output** that carries an `outcome` and optional `data`. Flows are plain files in `flows_dir` (by default `./config/flows`), so you can review them in a pull request and keep them in version control.

## Anatomy of a flow

```yaml
description: "Route a support ticket"      # shown in the TUI and in GET /v1/flows
defaults: { backend: ollama, model: nimble }  # used by nodes that ask a model
start: urgent                              # the first node
nodes:
  urgent:
    type: rule                             # no model involved
    when: 'input.priority == "high"'       # CEL over the request
    out: { "true": page, "false": billing }  # port -> next node or output
  billing:
    type: rule
    when: 'input.topic == "billing"'
    out: { "true": finance, "false": queue }
outputs:
  page:    { outcome: ok, data: { team: oncall,  ticket: "$input/id" } }
  finance: { outcome: ok, data: { team: finance, ticket: "$input/id" } }
  queue:   { outcome: ok, data: { team: support, ticket: "$input/id" } }
```

Save it as `config/flows/ticket-triage.yaml` and the daemon loads it without a restart. Ask it for a decision:

```sh
curl -s http://127.0.0.1:8080/v1/decide/ticket-triage \
  -H 'Content-Type: application/json' \
  -d '{"state":{"id":"T-42","priority":"high","topic":"login"}}'
```

```json
{"data":{"team":"oncall","ticket":"T-42"},"flow":"ticket-triage","outcome":"ok","path":[{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"urgent","port":"true"}],"trace_id":"18e2ca4a6312005c0002"}
```

The request's `state` is what your expressions see as `input`. The reply tells you the `outcome`, the output `data`, and the `path` the flow walked, one entry per node, so you can always see why a decision came out the way it did.

## Node types

| Type | What it does | Calls a model |
|---|---|---|
| `rule` | Evaluates a [CEL](https://cel.dev) expression over `input`. Ports: `true` and `false`. | No |
| `noul` | Asks a yes/no question and gets a probability. Split it with a `threshold`, or map probability ranges to ports. | Yes |
| `choice` | Asks the model to pick one label from `criteria`. Each label is a port. | Yes |
| `score` | Asks for a rating over an ordered list of `criteria`. Ports are numeric ranges such as `"0..1.5"`. | Yes |
| `batch` | Asks several questions in **one** backend call and routes on all the answers. | Yes (one call) |

Numeric ranges include their lower bound: a `score` of `1.5` takes the port `"1.5..3"`.

Nodes that ask a model take `instructions` (the question) and `criteria`, and inherit `backend` and `model` from `defaults` unless they name their own.

Besides your own ports, a model node can use two special ones:

- `error` — taken when the backend fails or times out. Without it, a failing node ends the request with `502`.
- `low-confidence` — taken when the model's confidence is below `min_confidence`.

A flow that routes work to a model by difficulty, from the repository's `config/flows/model-route.yaml`, uses all of this:

```yaml
nodes:
  tier:
    type: score
    instructions: "How difficult is the task?"
    criteria: ["trivial", "easy", "medium", "hard", "very hard"]
    out: { "0..1.5": small, "1.5..3": risky, "3..4": large, error: medium, low-confidence: medium }
  risky:
    type: noul
    instructions: "Does the task touch security, production data, or money?"
    out: { "true": large, "false": medium }
outputs:
  small:  { outcome: route, data: { model: haiku, effort: low } }
  medium: { outcome: route, data: { model: sonnet, effort: medium } }
  large:  { outcome: route, data: { model: opus, effort: high } }
```

## Outputs and references

An output has an `outcome` (any label you choose: `ok`, `allow`, `deny`, `route`…) and an optional `data` object that is returned to the caller. Inside `data` you can copy values with references:

| Reference | Value |
|---|---|
| `"$input/id"` | The field `id` of the request `state`. |
| `"$answer/<node>"` | The answer of a model node. |
| `"$answer/<node>/<question>"` | One answer of a `batch` node. |

## Batch: several questions, one call

A `batch` node avoids chaining several calls and lets you decide on all the answers together. Answers are available as `answers.<node>.<question>`:

```yaml
risks:
  type: batch
  questions:
    destructive:  { type: noul, instructions: "…", criteria: { "true": "…", "false": "…" } }
    exfiltration: { type: noul, instructions: "…", criteria: { "true": "…", "false": "…" } }
  route:                                   # ordered: the first `when` that is true wins
    - { when: "answers.risks.destructive >= 0.85", to: deny }
    - { when: "answers.risks.destructive >= 0.55", to: ask }
  else: allow                              # when no `when` matches
  out: { error: ask_error }                # optional: where a backend failure goes
```

## Reusable signals

When several flows ask the same question, declare it once as a **signal** in `config/signals/<name>.yaml` and name it from the flow. A signal is a node body with nothing wired: `type`, `instructions`, `criteria` or `when`, but never `out`, `backend` or `model`. The file name is the signal name.

- A **heuristic** signal (`type: rule`) is CEL over `input` only. Any `when` reads it as `signals.<name>`.
- A **learned** signal (`noul`, `choice`, `score`) is a question. A `batch` asks it with `signals: [name, …]`.

The bundled `config/flows/tool-risk-local.yaml` combines both: two fixed rules that run first, then four model questions in one call.

```yaml
# config/signals/rm_root.yaml — a heuristic
type: rule
when: 'has(input.tool_input.command) && input.tool_input.command.matches(r"rm\s+-[a-zA-Z]*[rf][a-zA-Z]*\s+(/|~|\$HOME)(\s|$)")'
```

```yaml
# in the flow
risks:
  type: batch
  signals: [destructive, exfiltration, privilege, secret_exposure]
  route:
    - { when: 'answers.risks.exists(k, answers.risks[k] >= 0.85)', to: deny }
    - { when: 'answers.risks.exists(k, answers.risks[k] >= 0.55)', to: ask }
  else: allow
```

A heuristic that fails sends its node to the `error` port; it is never read as `false`.

## Editing flows safely

- **Hot reload.** The daemon watches `flows_dir` and `signals_dir`. Saving a valid file replaces the flow; an invalid file keeps the previous version and is listed with its reason in `GET /v1/flows`.
- **Validate before you save.** Send the YAML to `POST /v1/flows/validate`, or run a draft once with `POST /v1/flows/draft/run`. Neither publishes anything. See [HTTP API](/en/api/).
- **Use the visual editor.** `jevalan edit config/flows/<flow>.yaml` opens a draft in the terminal and writes changes back as patches, so comments and ordering survive. It never writes a draft that does not validate.
- **Mind the live flows.** `config/flows/tool-risk-local.yaml` and `config/signals/` decide for real when the Claude Code hook is installed. Practise on a copy.
