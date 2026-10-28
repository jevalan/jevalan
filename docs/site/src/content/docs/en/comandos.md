---
title: Commands
description: Common commands for the TUI, daemon, flows, and lab.
---

| Command | Use |
|---|---|
| `jevalan tui --demo` | Open the TUI with a daemon, mock services, and simulated traffic. |
| `jevalan tui --fake` | Open the TUI with a deterministic sequence, without a daemon or backends. |
| `jevalan serve --config config.yaml` | Start the local daemon. |
| `jevalan edit config/flows/<flow>.yaml` | Edit a flow as a draft. |
| `jevalan lab check` | Validate lab fixtures offline. |
| `jevalan lab offline` | Replay fixtures offline. |
| `jevalan --version` | Show the installed version. |

`config/flows/tool-risk-local.yaml` participates in hook decisions. Practise on a copy before editing it.

Run `jevalan <command> --help` to see all options for a command.
