---
title: Architecture
description: How the proxy, the flow engine, the hub, the event bus, and the lab fit together.
---

Jevalan is a local daemon with three ways in, one engine in the middle, and a stream of events that the terminal UI watches.

```text
        agents, hooks, scripts
                │
   ┌────────────┼──────────────────────────┐
   │            │                          │
POST /{backend}/v1/systemone   POST /v1/decide/{flow}   POST /v1/hub
(raw System One question)      (one specific flow)      (by capability)
   │            │                          │
┌──▼────────────▼──────────────────────────▼─────────────────────┐
│                           JEVALAN                              │
│                                                                │
│  passthrough ─────────┐   flow engine ◄──── capability registry│
│  (forwards untouched) │   rule · noul ·      ├─ flow  → your flows
│                       │   choice · score ·   ├─ http  → a service
│                       │   batch              └─ exec  → a process
│                       ▼          │                             │
│           System One backends (OpenRouter, TypeSafe, Ollama,   │
│           local models): keys, deadlines, cost                 │
│                                                                │
│  event bus ──── WebSocket /events ────► terminal UI            │
└────────────────────────────────────────────────────────────────┘
                          ▲
                          │ same path, in process, no HTTP
                  jevalan lab (offline · record · live)
```

## Three principles

1. **The model judges, your code acts.** A model returns typed probabilities. Local rules and thresholds, which you can read and test, turn them into a decision. The agent applies the effect. Jevalan never performs it.
2. **The agent does not know who decides.** It asks for a capability. Whether your own flow or a third-party specialist answers is a choice in configuration, so you can change it without touching the agent.
3. **Telemetry is explicit.** Every step emits an event, and you can see it in the TUI. Events have no field for the original input, but they can carry answers, outcomes, signals, and errors, so treat the stream as sensitive.

## The pieces

| Piece | Role |
|---|---|
| **Passthrough** | Forwards a System One request to a backend untouched and holds the key, the deadline, and the cost accounting. |
| **Flow engine** | Walks a YAML graph of rules and model questions and returns an outcome with the path it took. |
| **Capability registry** | Maps a typed question to a provider: a flow, an HTTP service, or a local process. |
| **Event bus** | Publishes what happens, node by node, over a WebSocket. |
| **Terminal UI** | Shows the topology, metrics, decisions, and traces live, and edits flows as drafts. |
| **Lab** | Runs the same cases through the hub path, offline or against real backends. |
| **Hook adapters** | Let Claude Code and Crush ask `tool.risk` before every tool call. |

## A decision, end to end

Take a Claude Code session that is about to run `rm -rf /tmp/build`:

1. Claude Code runs the `PreToolUse` hook, which reads the tool call from stdin.
2. The hook finds the daemon through its discovery file and sends `POST /v1/hub` for `tool.risk`.
3. The hub resolves `tool.risk` to the `tool-risk-local` flow.
4. Two fixed rules run first: a recursive delete of the root or the home folder is denied, and a download piped into an interpreter asks. Neither calls a model.
5. If neither matches, one `batch` node asks four risk questions in a single call to a local model and gets four probabilities.
6. Routing rules apply the thresholds: any risk of 0.85 or more is `deny`, any of 0.55 or more is `ask`, otherwise `allow`.
7. The hub validates the decision against its schema and returns it. Every step was published as an event.
8. The hook records the decision in the shadow log. In `enforce` mode it also answers Claude Code; in `shadow` it stays silent.

The hook can only tighten Claude Code's permissions, never relax them. In the worst case it asks too much.

## Go deeper

- [Flows](/en/flujos/) and [Capabilities and hub](/en/capacidades/) explain the two ways to define a decision.
- [Lab](/en/laboratorio/) explains how to measure one.
- The repository has the full design with more detailed diagrams: [`docs/architecture.md`](https://github.com/jevalan/jevalan/blob/main/docs/architecture.md).
