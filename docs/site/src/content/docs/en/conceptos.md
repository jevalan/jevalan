---
title: Concepts
description: What Jevalan does and how the proxy, flows, and hub fit together.
---

## Local proxy

Jevalan exposes an HTTP daemon on the local machine. An agent calls Jevalan instead of connecting directly to a backend. The proxy can add the configured key and forwards the System One request.

## Three decision routes

| Route | Purpose |
|---|---|
| `POST /v1/systemone` | Send a System One request to the default backend. |
| `POST /v1/decide/{flow}` | Run a named YAML flow. |
| `POST /v1/hub` | Resolve a typed capability such as `tool.risk`. |

Flows and capabilities are loaded from the working directory and can be observed in the TUI. The `/events` route publishes events for the interface.

## Who performs the action

Jevalan returns a response or a decision. It does not perform the requested action, change the agent's files, or approve operations on its behalf. The client remains responsible.

## Backends and providers

A flow can use local rules, query a compatible backend, or ask a configured provider for a decision. The hub can also use a `flow`, `http`, or `exec` provider; process execution is disabled by default.
