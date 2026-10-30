---
title: Capability catalog
description: Which capabilities ship with Jevalan, what question each one answers, who consumes it and what to do when there is no answer.
---

This page is the inventory. For the mechanism (how a capability is requested, how it is defined, what states a reply has) see [Capabilities and hub](/en/capacidades/). Here is what exists and what does not.

Each capability carries one of these statuses:

| Status | Meaning |
|---|---|
| **Included** | It has a manifest, schemas, a flow or provider, and [lab](/en/laboratorio/) fixtures in the repository. |
| **Example flow** | It exists as a flow (`POST /v1/decide/{flow}`), but is not registered as a capability in the hub. |
| **Under study** | An idea, not implemented and with no date. |

## `tool.risk` (included)

**Question:** is this tool call risky? It answers `allow`, `ask` or `deny`.

| | |
|---|---|
| Input | `tool_name` and `tool_input` (required); `cwd`, `session_id`, `tool_use_id` and `transcript_path` (optional). |
| Decision | `verdict` (`allow`, `ask`, `deny`), `reason`, `source` and, when a model was involved, `probabilities`. |
| Deadline | 5000 ms by default. |
| Who serves it | `config/capabilities-local/tool.risk.yaml` decides it with the `tool-risk-local` flow, entirely locally. `config/capabilities/tool.risk.yaml` hands it to an external tool through an `exec` provider, which requires `hub.allow_exec: true`. |
| Who consumes it | The Claude Code and Crush [hooks](/en/integraciones/), and the `tool_risk` tool of the [MCP server](/en/servidor-mcp/). |
| No answer | The hook answers `ask`, never a blind `allow`. |

The local flow applies two fixed rules first (recursive deletion of the root or home directory, and a download piped into an interpreter). Only if neither fires does it ask the model four things in a single call: destruction, exfiltration, privilege and secret exposure. A risk of 0.85 or more is `deny`; 0.55 or more is `ask`; anything lower is `allow`. The rules and the four questions live in `config/signals/`, and any flow can reuse them.

## `agent.wakeup` (included)

**Question:** an agent is asleep waiting for something and an event arrived: is it worth waking it?

| | |
|---|---|
| Input | `waiting_for` and `skipped` (required); `event`, `observation`, `from_user` and `max_skips` (optional). |
| Decision | `wake` (boolean), `reason` and, when a model was involved, `probability`. |
| Deadline | 3000 ms by default. |
| Who serves it | The `agent-wakeup` flow, through `config/capabilities/agent.wakeup.yaml`. |
| Who consumes it | Calls to `POST /v1/hub` and the `agent_wakeup` tool of the MCP server. |
| No answer | If the model fails, the flow itself wakes the agent (`reason: error`): it prefers one wake-up too many to an agent asleep forever. |

The possible reasons are `user-message`, `nothing-to-judge`, `skip-limit`, `judged`, `unsure` and `error`. The first three are deterministic rules and consult no model: a user message always wakes, a notice with no event and no observation also wakes, and after `max_skips` consecutive skips (10 by default) it wakes anyway. Only the rest reaches the model.

## `agent.stop` (included)

**Question:** the agent says it is done: may its work be considered finished? It is the "done gate": it asks for evidence before letting the agent stop.

| | |
|---|---|
| Input | `task` and `blocked` (required); `claim`, `evidence`, `tests_passed`, `files_changed` and `max_blocks` (optional). `blocked` is how many times it has already been prevented from stopping. |
| Decision | `stop` (boolean), `reason` and, when a model was involved, `probability`. |
| Deadline | 3000 ms by default. |
| Who serves it | The `agent-stop` flow, through `config/capabilities/agent.stop.yaml`. |
| Who consumes it | Calls to `POST /v1/hub` and the `agent_stop` tool of the MCP server. There is no `Stop` hook adapter yet: whoever uses it decides what to do with `stop: false`. |
| No answer | If the model fails, the flow itself lets the agent stop (`reason: error`): a fallen guard must not trap it. If the hub gives no answer, do the same in your consumer. |

The possible reasons are `block-limit`, `no-changes`, `tests-failed`, `verified`, `unverified`, `unsure` and `error`. The first rules are deterministic and are evaluated in this order: after `max_blocks` consecutive blocks (3 by default) it always lets the agent stop, even with red tests, so the agent is not trapped; with no changed files there is nothing to verify; with failing tests it cannot stop. Only the rest reaches the model, which judges whether the `evidence` demonstrates the task: below 0.3 the agent continues (`unverified`), between 0.3 and 0.6 it also continues (`unsure`, because with doubt it does not stop) and from 0.6 it may stop (`verified`).

## Example flows

These two flows live in `config/flows/` and run with `POST /v1/decide/{flow}`. They are not hub capabilities: they have no input or decision schema, so the hub neither validates them nor exposes them over MCP. Turning one into a capability means writing it a manifest, as explained below.

| Flow | What it decides | Result |
|---|---|---|
| `agent-guard` | A guard for agent commands: does it destroy data outside the project? What kind of task is it? How risky is it? | `allow`, `deny`, `ask-user` or `abstain`. |
| `model-route` | Which model and reasoning effort a task deserves, by difficulty and whether it touches security, production or money. | `route`, with `model` and `effort`. |

Example 4 in [Examples](/en/ejemplos/) walks through `model-route`.

## Under study

None of this is implemented or dated. It is listed so it is clear what is product and what is not.

| Idea | Question | Why it fits |
|---|---|---|
| `model.route` | Which model and effort does this task deserve? | It already exists as a flow; it would need a manifest and schemas. |

## Adding a capability

1. Write the flow in `config/flows/` (with [`jevalan edit`](/en/comandos/), not by hand; see [Flows](/en/flujos/)).
2. Write the two JSON schemas in `schemas/` next to the manifest: `<capability>.input.json` and `<capability>.decision.json`.
3. Write the manifest `<capability>.yaml` in `hub.capabilities_dir`, with `provider: { id: jevalan-flow, flow: <flow> }`.
4. Add fixtures in `config/lab/fixtures/<capability>/` and pass `jevalan lab check` and `jevalan lab offline`.
5. Decide the consumer's no-answer behavior **before** plugging it in (see the [guide](/en/guia-decisiones/)).

If something fails to load, `GET /v1/hub/capabilities` lists it under `invalid` with the reason; a `flow` capability also becomes invalid while its flow is missing.
