---
title: MCP server
description: Let any MCP client ask Jevalan for decisions, and list, validate and try flows, with one command.
---

`jevalan mcp` is a small bridge between the [Model Context Protocol](https://modelcontextprotocol.io) and the daemon's HTTP API. An MCP client such as Claude Code starts it over stdio, and every capability the hub has loaded shows up as a tool. It adds no decision logic of its own: each call is forwarded to the daemon, which keeps applying its deadlines, validation and events.

## Connect a client

The daemon must be running (`jevalan serve`). The bridge finds it the same way the hooks do: through the discovery file (`daemon.json`), or the default address if there is none.

For Claude Code, add it to your project's `.mcp.json`, with the **absolute path** to a copy of the binary:

```json
{
  "mcpServers": {
    "jevalan": { "command": "/path/to/jevalan", "args": ["mcp"] }
  }
}
```

| Option | Meaning |
|---|---|
| `--addr host:port` | Talk to this daemon only, ignoring `daemon.json`. |
| `--token` | The daemon's token (also read from `JEVALAN_TOKEN`). Never printed. |
| `--allow-live` | Let `jevalan_try_draft` use the `live` and `record` transports. See below. |

## Tools

**One per capability.** Each loaded capability becomes a tool whose input schema is the capability's own. The name is the capability id with every character outside letters, digits, `_` and `-` replaced by `_`, so `tool.risk` is `tool_risk` and `agent.wakeup` is `agent_wakeup`. The result is the hub's typed answer: `status` is `ok`, `abstain` or `error`, and only `error` is flagged as a failed call. When the daemon reloads capabilities, the client is told the list changed.

**Four for flows**, always present:

| Tool | What it does |
|---|---|
| `jevalan_run_flow` | Runs a published flow by name with a `state`, and returns its `outcome`, `data` and `path`. |
| `jevalan_list_flows` | Lists loaded flows and the invalid ones, with the reason. |
| `jevalan_validate_flow` | Validates a flow's YAML with the real parser, without publishing it. An invalid flow answers `ok: false` with the problem; that is a verdict, not a failed call. |
| `jevalan_try_draft` | Runs a draft once without publishing it. |

## What it deliberately does not do

- It cannot save, edit or delete flows, nor touch `config/signals/`. Changing a flow stays a human job, done with [`jevalan edit`](/en/comandos/). `tool-risk-local` decides for real when the Claude Code hook is installed, and an agent rewriting its own safety rules would defeat the point.
- It never starts processes, and it opens no network port of its own.
- If the daemon is missing or stops answering, the call comes back as an error with the reason. It never invents a verdict.

## About `--allow-live`

`jevalan_try_draft` takes a `transport`. By default only `replay` is offered: it replays recorded answers and calls nothing. `live` and `record` call your real backends, which can cost money, so they are refused unless you start the bridge with `--allow-live`.

## Ask yourself before you ask the agent

An agent can now check a call with `tool_risk` before making it. That is a convenience, not a guard: a model that is asked politely can also decide not to ask. For enforcement, keep using the [hook](/en/integraciones/).
