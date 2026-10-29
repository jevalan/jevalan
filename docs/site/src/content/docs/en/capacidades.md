---
title: Capabilities and hub
description: Ask for a typed decision by name, without knowing who answers it.
---

A **capability** is a named, typed question such as `tool.risk` ("is this tool call risky?") or `agent.wakeup` ("is it worth waking this agent?"). Your agent asks the hub for the capability; which flow, process, or service answers it is decided in configuration. You can swap the provider without changing the agent.

## Ask for a decision

```sh
curl -s http://127.0.0.1:8080/v1/hub \
  -H 'Content-Type: application/json' \
  -d '{"protocol_version":"jev-hub/0.1","request_id":"r1","capability":"agent.wakeup","capability_version":"1","input":{"waiting_for":"CI to finish","from_user":true,"skipped":0}}'
```

```json
{"capability":"agent.wakeup","capability_version":"1","decision":{"reason":"user-message","wake":true},"protocol_version":"jev-hub/0.1","provider":{"id":"jevalan-flow","version":"0.1.0"},"request_id":"r1","status":"ok","trace":{"cost":"not_measured","elapsed_ms":2,"path":[{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"user","port":"true"}],"trace_id":"18e2ca4a7cd8c9d00005"}}
```

| Request field | Meaning |
|---|---|
| `protocol_version` | Always `jev-hub/0.1`. |
| `request_id` | Your identifier; it is echoed in the reply. |
| `capability`, `capability_version` | Which question, and which version of it. |
| `input` | The data to judge. It must satisfy the capability's `input_schema`. |
| `constraints.deadline_ms` | Optional. It can only shorten the capability's default deadline. |

## The reply

The reply always has a `status`:

| Status | Meaning | `decision` present |
|---|---|---|
| `ok` | The provider decided. | Yes, valid against the `decision_schema`. |
| `abstain` | The provider declined to give an opinion. | No |
| `error` | Something failed, with a stable `error.code`. | No |

Your agent should treat `abstain` and `error` as "no answer" and choose a safe default; they never carry a `decision`. For example, an unknown capability:

```json
{"capability":"nope.nope","capability_version":"1","error":{"code":"unknown_capability","message":"capacidad desconocida: nope.nope","retryable":false},"protocol_version":"jev-hub/0.1","provider":{"id":"-","version":"-"},"request_id":"r2","status":"error","trace":{"cost":"not_measured","elapsed_ms":0,"trace_id":"18e2ca4a825001080006"}}
```

Other codes you may see are `invalid_input` (the `input` does not match the `input_schema`), `version_mismatch` (the `capability_version` is not the loaded one), `provider_failed`, `deadline_exceeded`, `provider_disabled`, and `invalid_decision`. `retryable` tells you whether trying again can help.

The HTTP status is `200` even for these errors, because they travel in `status`. Only a malformed request, such as a `protocol_version` other than `jev-hub/0.1`, answers `400`.

## Define a capability

A capability is a YAML file in `hub.capabilities_dir`. The file name is the capability name (`agent.wakeup.yaml`):

```yaml
manifest_version: "2"
namespace: local.jev
version: "1"
description: "Is it worth waking a sleeping agent?"
input_schema:    { $file: "schemas/agent.wakeup.input.json" }     # JSON Schema, inline or in a file
decision_schema: { $file: "schemas/agent.wakeup.decision.json" }
provider: { id: jevalan-flow, flow: agent-wakeup }                 # who answers it
defaults: { deadline_ms: 3000 }
```

Both schemas are enforced: a request that does not match `input_schema` is rejected, and a provider reply that does not match `decision_schema` becomes `error`.

## Providers

| Provider | Key | Who answers |
|---|---|---|
| Flow | `flow` | A [flow](/en/flujos/) of your own, running inside Jevalan. |
| Http | `http` | A POST to a `url`. |
| Exec | `exec` | A local process: `command`, plus the environment variable names listed in `env`. |

`abstain_when` and `error_when` are optional expressions over the provider's `response` that turn a reply into `abstain` or `error`. They let you adapt a third-party tool without changing it. If an expression cannot be evaluated, for example because it reads a field the response does not have, the reply is an `error` with the code `provider_failed`.

### Running processes is off by default

`exec` providers launch a program on your machine, so they are disabled unless you set `hub.allow_exec: true` (or `JEVALAN_ALLOW_EXEC=1`). When they run:

- The process is started **without a shell**, with an empty environment plus `PATH` (and `SystemRoot` on Windows) and only the variables named in `env`.
- It is killed, with its whole process tree, if it exceeds the deadline.
- Its output is capped at 1 MiB.
- On Windows, `.cmd` and `.bat` files are refused, because launching them goes through a shell.
- The command, its environment, and its URL never appear in events, logs, or `GET /v1/hub/capabilities`.

With `allow_exec` off, an `exec` capability stays valid but answers `provider_disabled`.

## List what is loaded

```sh
curl -s http://127.0.0.1:8080/v1/hub/capabilities
```

The reply lists each capability with its version, description, provider type, schemas, and default deadline, plus an `invalid` object explaining any that failed to load. The directory is watched: a valid save replaces the capability, an invalid one keeps the previous version, and a `flow` capability becomes invalid while its flow is missing.
