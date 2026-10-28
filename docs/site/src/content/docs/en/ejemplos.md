---
title: Examples
description: Five worked examples, from a rules-only flow to a guard on your coding agent and a check in CI.
---

Each example is self-contained. The first two were run against a real daemon and the responses shown are its actual output; the rest use files and commands that ship with the repository.

## 1. Route support tickets with plain rules

No model, no backend call: the flow only reads the request and follows ports. Save this as `config/flows/ticket-triage.yaml`:

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

The answer is `outcome: ok` with `data: {"team":"finance","ticket":"T-43"}`, plus the `path` that led there. See [Flows](/en/flujos/) for every node type.

## 2. Gate a deployment

The same idea, used as a safety check before a pipeline step. Two rules decide, and the flow returns `allow`, `ask` or `deny` with a reason:

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

| Request `state` | `outcome` | `data` |
|---|---|---|
| `{"env":"prod","branch":"feature/x","tests_passed":true}` | `deny` | `{"reason":"production deploys must come from main"}` |
| `{"env":"prod","branch":"main","tests_passed":false}` | `ask` | `{"branch":"main","reason":"tests have not passed"}` |
| `{"env":"prod","branch":"main","tests_passed":true}` | `allow` | none |

Your pipeline reads `outcome` and decides what to do; Jevalan never runs the deployment.

## 3. Guard your coding agent

Put the hook in `shadow` mode first. It records what Jevalan would have decided and never interferes:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "*", "hooks": [ { "type": "command", "command": "/path/to/jevalan hook claude-code --mode shadow" } ] }
    ]
  }
}
```

Work normally for a few days, then read what it saw:

```sh
jevalan shadow report --since 2026-10-01 --out report.md
```

When the verdicts match your own judgement, change `--mode shadow` to `--mode enforce`. In `enforce` the hook can only tighten permissions, and when it cannot decide it asks. See [Integrations](/en/integraciones/).

## 4. Pick a model by difficulty

`config/flows/model-route.yaml` scores a task and sends easy ones to a small model and hard ones to a large one, with a yes/no check for anything touching security, production data or money:

```yaml
start: tier
nodes:
  tier:
    type: score
    instructions: "How difficult is the task?"
    criteria: ["trivial", "easy", "medium", "hard", "very hard"]
    out: { "0..1.5": small, "1.5..3": risky, "3..4": large, error: medium, low-confidence: medium }
  risky:
    type: noul
    instructions: "Does the task touch security, production data or money?"
    out: { "true": large, "false": medium }
outputs:
  small:  { outcome: route, data: { model: haiku,  effort: low } }
  medium: { outcome: route, data: { model: sonnet, effort: medium } }
  large:  { outcome: route, data: { model: opus,   effort: high } }
```

Unlike the first two, this flow asks a model, so it needs a reachable backend. Note the `error` and `low-confidence` ports: when the model fails or is unsure, the flow falls back to the middle option instead of failing the request. The file in the repository has its instructions in Spanish; the structure is the same.

## 5. Check decisions in CI

Add the laboratory to your pipeline. It needs no backend and no key:

```sh
jevalan lab check && jevalan lab offline
```

`lab check` validates the fixtures (it prints `lab en orden: 16 fixtures` in this repository). `lab offline` replays them and prints a table per capability with `pasa`, `skipped` and `fallan` columns; it exits non-zero if any case fails.

To validate a flow file before it reaches the daemon, post it to `POST /v1/flows/validate`. An invalid flow answers `ok: false` with the exact problem, including the position inside a CEL expression. See [API](/en/api/) and [Laboratory](/en/laboratorio/).
