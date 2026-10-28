---
title: Integrations
description: Connect Jevalan to Claude Code and Crush with hooks, and learn from what they record.
---

Jevalan includes hook adapters. A hook is a small program that the client runs **before every tool call**: it sends the call to the local hub, asks for the `tool.risk` capability, and translates the answer into the format the client expects. Nothing is executed by Jevalan; the client keeps the final word.

## Shadow and enforce

Every hook runs in one of two modes, chosen with `--mode`:

| | `shadow` (default) | `enforce` |
|---|---|---|
| Prints to the client | Nothing, and always exits 0 | A verdict when it is stricter than the client's own |
| Effect on the session | None: the client carries on exactly as before | `deny` blocks, `ask` makes the client ask you |
| Records the decision | Yes, in `shadow.jsonl` | Yes |

In `enforce`, the hook can only **tighten** permissions, never relax them:

| Verdict | What the hook does |
|---|---|
| `deny` | Blocks the tool and shows the reason. |
| `ask` | Makes the client ask you. |
| `allow` | Stays silent: the client's own permissions still apply. |
| no answer (no daemon, abstain, error, timeout, bad input) | Answers `ask`, never a blind `allow`. |

Start in `shadow`, read the report for a few days, and only then switch to `enforce`.

## Claude Code

Add the hook to `~/.claude/settings.json`, with the **absolute path** to a copy of the binary (never point it at `target/`, so a `cargo clean` cannot break your sessions):

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "*",
        "hooks": [
          { "type": "command", "command": "/path/to/jevalan hook claude-code --mode shadow" }
        ]
      }
    ]
  }
}
```

Start the daemon with `jevalan serve`. Once it listens, it writes a discovery file (`daemon.json`, with the address and token) in `%LOCALAPPDATA%/jevalan` on Windows or `$HOME/.jevalan` elsewhere, and the hook finds it from any folder. If the daemon is not running, the hook finds no file and does not even try to connect.

The full guide, including how to update the installed binary on Windows, is in the repository: [Install the Claude Code hook](https://github.com/jevalan/jevalan/blob/main/docs/claude-code-hook-install.md).

## Crush

Crush hooks are compatible with Claude Code's. In your `crushrc`, one line:

```sh
hook add PreToolUse --name jevalan --matcher '^(bash|edit|write|multiedit)$' --command '"$LOCALAPPDATA/jevalan/bin/jevalan.exe" hook run --client crush --dialect crush --mode shadow' --timeout 8
```

`jevalan hook run --client <name> --dialect <claude|crush>` is generic: supporting another client changes flags, not code. Crush only tells `allow` from `deny`, so in `enforce` it blocks or lets its own permissions decide. See [Install the Crush hook](https://github.com/jevalan/jevalan/blob/main/docs/crush-hook-install.md).

## Privacy: use local models

A hook is **global**: every tool call of every project on your machine, including client repositories and `.env` files, goes to the daemon, and the daemon passes it to whatever serves `tool.risk`. Redaction in the shadow log protects only the local file; **what the daemon forwards goes out as it is**.

So, until an outbound shield exists:

- Serve `tool.risk` with a **local** model, as `config/flows/tool-risk-local.yaml` does. Nothing leaves your machine.
- Do **not** point the hook's daemon at a cloud backend or a third-party specialist.

The shield you would need is: skip the query when the tool touches sensitive paths (`.env*`, `*.pem`, `id_rsa*`, kubeconfig…), redact the outgoing input too, sanitize the transcript, allow only listed folders, and validate the redaction patterns against your own repositories.

## What the hook records

Each decision is appended to `shadow.jsonl` in the same data directory, with secrets of known shapes (`sk-…`, `ghp_…`, `AKIA…`, `Bearer …`, `key=value`) replaced by `[REDACTED]`. The file stays on your machine, and it still holds the commands, so do not upload it.

Turn it into something you can read, and then into tests:

```sh
# A Markdown report: verdicts, why the asks happened, probabilities, latency
jevalan shadow report --since 2026-10-01 --out report.md

# The same, plus a redacted excerpt of each example (local use only)
jevalan shadow report --with-inputs

# Real decisions as lab fixtures, highest risk first
jevalan shadow fixtures --lab-dir lab-fixtures-batch --max 20
```

By default the report holds no tool input, so you can publish it. The fixtures feed the [lab](/en/laboratorio/), so your test bench grows out of what actually happened.

In the terminal UI, press `H` to see the decisions live: counters with p50 and p95 latency, the latest calls, and the probability of each question as bars with ticks at the 0.55 and 0.85 thresholds. It reads the file locally and never shows the input, so it is safe to screenshot.

## Uninstall

Remove the `jevalan` entry from `PreToolUse` in `~/.claude/settings.json` (or disable it from `/hooks`), or run `hook remove PreToolUse --name jevalan` in Crush.
