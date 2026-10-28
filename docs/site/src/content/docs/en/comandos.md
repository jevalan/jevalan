---
title: Commands
description: The commands of the TUI, daemon, flow editor, hooks, shadow log, and lab.
---

| Command | Use |
|---|---|
| `jevalan tui --demo` | Open the TUI with a daemon, mock services, and simulated traffic. No keys needed. |
| `jevalan tui --fake` | Open the TUI with a deterministic sequence, without a daemon or backends. |
| `jevalan tui` | Attach to a daemon that answers, or start one against your real backends. |
| `jevalan serve --config config.yaml` | Start the local daemon. |
| `jevalan edit config/flows/<flow>.yaml` | Edit a flow as a draft. |
| `jevalan hook claude-code [--mode shadow\|enforce]` | The `PreToolUse` hook for Claude Code. |
| `jevalan hook crush [--mode shadow\|enforce]` | The hook for Crush. |
| `jevalan hook run --client <name> --dialect <claude\|crush>` | The generic hook adapter. |
| `jevalan shadow report` | A Markdown report over the shadow log. |
| `jevalan shadow fixtures` | Turn real decisions into lab fixtures. |
| `jevalan mcp [--allow-live]` | MCP server over stdio: exposes the hub's capabilities as tools. See [MCP server](/en/servidor-mcp/). |
| `jevalan lab check` | Validate lab fixtures offline. |
| `jevalan lab offline` | Replay fixtures offline. |
| `jevalan lab record` | Call real backends and record cassettes, within a budget. |
| `jevalan lab live` | Measure real backends with repetitions, within a budget. |
| `jevalan --version` | Show the installed version. |

Run `jevalan <command> --help` to see all options of a command.

## Terminal UI options

| Flag | Meaning | Default |
|---|---|---|
| `--url` | The daemon's events WebSocket. | `ws://127.0.0.1:8080/events` |
| `--token` | The daemon's token (also `JEVALAN_TOKEN`). | none |
| `--fx` | Visual effects: `off`, `events`, or `full` (also `JEVALAN_FX`). | `events` |
| `--fake` | Feed the TUI with simulated traces. | off |
| `--fake-rate`, `--fake-seed` | Simulated requests per second, and a seed for repeatable runs. | `2`, random |
| `--demo` | Mock backends and simulated traffic. | off |
| `--config` | Config file for the daemon the TUI starts, if it has to. | none |

### Keys

| Key | Action |
|---|---|
| `q` | Quit. |
| `?` | Help. |
| `p` | Pause or resume. |
| `f` | Cycle the effects: off, events, full. |
| `esc` | Back. |
| `tab` | Switch focus between the topology and the feed. |
| `enter` | Open the flow (on a topology port) or the trace (on a feed row). |
| arrows or `hjkl` | Move the view. |
| `+` `-` `0` | Zoom in, zoom out, fit to view. |
| `n` `N` | Next or previous node. |
| `t` | In a trace, toggle between the node path and the event timeline. |
| `H` | Live view of the hook decisions. |
| `x` | In `--demo`, knock down or bring back the selected backend. |

## Flow editor keys

`jevalan edit` is a terminal editor over one flow file. It works on a draft: it never publishes, never touches the daemon's registry, and a draft that does not validate is never written to disk.

| Key | Action |
|---|---|
| `tab` | Move between the graph, the inspector, and the bottom band. |
| arrows or `hjkl` | Move the selection. |
| `e` | Edit the selected field. `enter` confirms it in the draft. |
| `a` | Add a node: `rule`, `noul`, `choice`, `score`, or `batch`. |
| `d` | Delete the selected node, with confirmation. |
| `c` | Rewire the selected port. |
| `s` | Save by patches, keeping a `.bak` copy. Comments and ordering survive. |
| `g` | Save aside as `<name>.editado.yaml` when the file changed underneath. |
| `r` | Reload from disk, discarding the draft. |
| `q` | Quit, warning if there are unsaved changes. |
| `esc` | Cancel the current edit, menu, or confirmation. |

`config/flows/tool-risk-local.yaml` takes part in hook decisions. Practise on a copy before editing it.
