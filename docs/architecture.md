# jevalan architecture

*[Versión en español](es/architecture.md)*

This document explains, with diagrams, how the pieces of jevalan fit together and why.

## 1. Overview

jevalan is a **local decision hub**. Agents (Claude Code, Crush, scripts…) ask it; jevalan decides — or has a specialist decide — and returns a typed answer. It never performs the effect: that stays with whoever asked.

```
            ┌───────────── agents, hooks, scripts ──────────────┐
            │                                                    │
   POST /{backend}/v1/systemone   POST /v1/decide/{flow}    POST /v1/hub
   (raw System One question)      (one specific flow)       (by capability, jev-hub/0.1)
            │                          │                         │
╔═══════════▼══════════════════════════▼═════════════════════════▼══════════════╗
║                                   JEVALAN                                   ║
║                                                                             ║
║   passthrough ──────────────┐    flow engine ◄──────────── capability       ║
║   (forwards untouched)      │    (rule · noul · choice ·   registry         ║
║                             │     score)                   config/capabilities/    ║
║                             │         │                    ├─ flow ─────────╢──► flows in config/flows/
║                             │         │                    ├─ exec ─────────╢──► specialist process
║                             │         │                    └─ http ─────────╢──► specialist service
║                             ▼         ▼                                     ║
║                       System One backends (Jev via OpenRouter, TypeSafe,    ║
║                       Ollama…) — key custody, deadlines, cost               ║
║                                                                             ║
║   event bus ──────── WebSocket /events ────► TUI (live dashboard)           ║
╚═════════════════════════════════════════════════════════════════════════════╝
                                  ▲
                                  │ same path, in process, no HTTP
                         jevalan lab (offline / record / live)
```

Three principles run through everything:

1. **Jev judges, code acts.** A System One model returns typed probabilities; local rules and thresholds turn those probabilities into a decision; the agent applies the effect.
2. **The agent does not know who decides.** It asks for a capability (`tool.risk`, `agent.wakeup`); whether an in-house flow or a third-party specialist serves it is chosen in configuration.
3. **Telemetry is explicit.** Events have no field for the original input, but they can carry answers, outcomes, signals, definitions and errors; treat the WebSocket as sensitive data.

## 2. A third-party `exec` provider: Toolgate (integration example)

This example describes `config/capabilities/tool.risk.yaml`, which launches Toolgate and may send inputs to remote services. **It is not the recommended hook configuration**: Claude Code and Crush should use `config/lab/config.local.yaml` and the local `tool-risk-local` flow; do not point the hook at Toolgate or at cloud models until outbound redaction exists.
Toolgate is a tool firewall for agents. In jevalan it acts as a **specialist**: jevalan launches it as a process, passes it the input and classifies its answer. The command comes from configuration, with `${VAR}` resolved when the capability loads (environment or `.env`): `node ${TOOLGATE_CLI} decide`.

```
 YOUR AGENT (Claude Code, Crush, a script…)
   │  POST /v1/hub  { capability: "tool.risk", input: {tool_name: "Bash", command: "git status"} }
   ▼
╔══════════════════════════ JEVALAN (our hub) ═══════════════════════════════╗
║  1. jev-hub/0.1 envelope ok?           4. deadline = min(requested, 5000 ms)║
║  2. capability "tool.risk" v1 exists?  5. which provider serves it?        ║
║  3. input meets its JSON Schema?          → config/capabilities/tool.risk.yaml   ║
║                                            provider: exec "toolgate"      ║
║      ┌────────────────────────────────────────────────┐                   ║
║      │ launches a process with NO shell, clean env     │                   ║
║      │ (only PATH + the keys in the `env` list)        │                   ║
║      │ writes the input to stdin ──────────────────┐   │                   ║
║      └─────────────────────────────────────────────│───┘                   ║
╚════════════════════════════════════════════════════│═══════════════════════╝
                                                     ▼
      ┌──────────────────── TOOLGATE (node process, third-party specialist) ──┐
      │  a) fixed rules (regex)  ── rm -rf / → DENY   curl|sh → ASK  ──┐      │
      │        │ none matches                                          │      │
      │        ▼                                                       │      │
      │  b) one call to Jev with 7 yes/no questions ───────────────────────────►  OPENROUTER
      │       destroys? exfiltrates? privileges? secrets?              │      │       Decisions API
      │       off-task? breaks a constraint? reserved decision?  ◄─────────────  Jev: p of each
      │       + authorized by the task?                                │      │
      │        ▼                                                       │      │
      │  c) thresholds: max risk ≥ 0.85 → deny · ≥ 0.55 → ask · else → allow  │
      │     (softens if authorized, never for secrets)                 │      │
      │        ▼                                                       ▼      │
      │  stdout: {verdict, reason, source, probabilities}                     │
      └──────────────────────────────────────┬────────────────────────────────┘
                                             ▼
╔════════════════════════════════ JEVALAN ═══════════════════════════════════╗
║  6. source = no-opinion? → ABSTAIN   source = fail-mode? → ERROR         ║
║  7. does the decision meet its JSON Schema? if not → ERROR invalid_decision║
║  8. HUB_REQUEST / HUB_RESPONSE ──────────────────────────► live TUI      ║
║     Events may contain answers, config and errors.                       ║
╚═══════════════════════════════════════════╤═════════════════════════════════╝
                                            ▼
 YOUR AGENT ◄─ { status: "ok", decision: {verdict: "allow", …}, provider: "toolgate", trace }
             and it decides what to do: run it, ask you, or not do it
```

Guarantees on jevalan's side: the process never goes through a shell, does not inherit the daemon's environment, is killed together with its whole tree if it exceeds the deadline, its output is capped at 1 MiB, and `exec` is disabled unless `hub.allow_exec: true`. `abstain` and `error` never carry a `decision`.

## 3. A capability served by an in-house flow

### 3.1 `agent.wakeup` (today)

Reimplements Wakegate inside jevalan: three rules and a single question to Jev.

```
 YOUR AGENT ─► JEVALAN ─ capability agent.wakeup ─ provider: flow "agent-wakeup"
                 │
                 ├─ rule  "input.from_user"                         ─ true ─► wake up (user-message)
                 ├─ rule  "!has(input.event) && !has(input.observation)" ─ true ─► wake up (nothing-to-judge)
                 ├─ rule  "input.skipped >= max_skips (10)"         ─ true ─► wake up (skip-limit)
                 ├─ noul  "does the event justify waking up?" ───────────────────►  OPENROUTER / Jev
                 │                                                  ◄────────────  p
                 ├─ p < 0.3        ─► sleep (judged)
                 ├─ 0.3 ≤ p < 0.5  ─► wake up (unsure)
                 ├─ p ≥ 0.5        ─► wake up (judged)
                 └─ Jev error      ─► wake up (error) — when in doubt, wake up
```

The sleep threshold went from 0.2 to 0.3 after the first real run of the lab (2026-10-09): an unrelated event fell right on the boundary.

### 3.2 `tool.risk` as a native flow (implemented)

Toolgate is, inside, another "decide": fixed rules, questions to Jev and thresholds. The in-house flow is already written and is the one that serves `tool.risk` locally: `config/flows/tool-risk-local.yaml`, with the capability pointing to it through `flow` in `config/capabilities-local/tool.risk.yaml`.

```
 YOUR AGENT ─► JEVALAN ─ capability tool.risk ─ provider: flow "tool-risk-local"
                 │
                 ├─ rule  "rm -rf /"                          ─ true ─► deny
                 ├─ rule  "curl … | sh"                       ─ true ─► ask
                 ├─ batch of 4 questions in ONE call ─────────────────►  KEV
                 │     destructive · exfiltration · privilege          ◄────────  p of each
                 │     secret_exposure
                 ├─ route "any ≥ 0.85"  ─ true ─► deny
                 ├─ route "any ≥ 0.55"  ─ true ─► ask
                 └──────────────────────────────────────────► allow
```

The `batch` node asks several questions in a single System One request and distributes the answers with its `route` rules, evaluated in order; CEL sees `answers.<node>.<question>`. It used to be up to four chained calls — cut short at the first risk ≥ 0.55 — and is now a single one, so the threshold applies to the whole set and not to the first one that shows up.

## 4. Third-party specialist versus in-house flow

| | Specialist (`exec`/`http`) | In-house flow (`flow`) |
|---|---|---|
| Who holds the recipe | the specialist (Toolgate) | us (a YAML in `config/flows/`) |
| Calibration | theirs, evaluated by its authors | ours, measured with the lab |
| Dependencies | the specialist's runtime (Node for Toolgate) | none |
| Visibility in the TUI | a chip for the specialist | the graph, node by node |
| Cost per decision | whatever the specialist does (Toolgate: 1 call with 7 questions) | one call per Jev node; with a `batch` node, **one** for several questions |
| What it ships with | task context, secret redaction, action ledger, audit | whatever we write |

The two forms are interchangeable for the agent: the capability is requested the same way and the provider is switched in `config/capabilities/<name>.yaml`. The lab lets you run the same fixtures through both and compare whether they decide equally well.

## 5. The lab

```
 config/lab/fixtures/<capability>/<case>.yaml          config/lab/lab.yaml (targets, budget, repetitions)
 (input + CEL assertions with their `why`)                │
          │                                               │
          ▼                                               ▼
 ┌─────────────────────────── jevalan lab ───────────────────────────┐
 │  offline: recorded answers (manual replay or cassette), no network│
 │  record:  real calls that are recorded as cassettes               │
 │  live:    real calls, fixtures × targets × repetitions            │
 │                   │                                               │
 │                   ▼                                               │
 │   hub::execute(..., Transport::{Replay | Record | Live})          │
 │   — the SAME path as POST /v1/hub, in process                     │
 └──────────────────────────────┬────────────────────────────────────┘
                                ▼
 .local/runs/<id>/ manifest.json · results.jsonl · report.md
 (pass, stable, abstain/error, p50/p95, real cost, Brier calibration)
```

`offline` is the regression suite for our code (deterministic, in CI); `live` measures the real quality of a backend or a specialist against a declared budget.

## 6. Claude Code as a client: the `PreToolUse` hook

Claude Code notifies jevalan **before** each tool. The hook is a subcommand of the binary itself, `jevalan hook claude-code`, installed in Claude Code's global configuration; it asks the daemon for the `tool.risk` capability and, depending on the mode, stays silent or answers.

```
 CLAUDE CODE (any session)
   │  is about to use a tool → PreToolUse hook
   │  stdin: {tool_name, tool_input, cwd, session_id, tool_use_id, transcript_path}
   ▼
 jevalan hook claude-code [--mode shadow|enforce]        (short-lived process, stateless)
   │  is there a daemon? → discovery file that `serve` leaves on startup
   │     none  ─► shadow: stays silent, exits 0      enforce: answers ASK
   │
   │  POST /v1/hub { capability: "tool.risk", input: {…, transcript_path} }
   ▼
╔══════════════════ JEVALAN (daemon) ══════════════════╗
║  tool.risk ─► tool-risk-local ─► local Kev/Laya         ║──► events ──► TUI
║  local flow: four risk questions in one call            ║
╚════════════════════════╤══════════════════════════════╝
                         ▼
 jevalan hook claude-code
   │  records the decision in the shadow log (local, secrets of a known shape redacted)
   │  ──► later: real fixtures for the lab come from there
   │
   ├─ SHADOW mode (default) ─► answers nothing: Claude carries on exactly as before
   └─ ENFORCE mode          ─► deny  → blocks, with the capability's reason
                                ask   → Claude asks you
                                allow → does NOT answer (never skips your permissions)
                                error/abstain/deadline → ASK, never a blind allow
```

The TUI (`jevalan tui`, key `H`) reads `shadow.jsonl` locally. The event carries no `input` field, but its answers (`NodeEnd.answer`), outcomes (`DecisionMade.outcome`), signals (`NodeAudit.signals`), definitions (`FlowLoaded.definition`) and errors may contain sensitive data or reproduce content. Do not share captures of the bus without reviewing them.

The event format is v2 (`src/crates/jevalan-events/src/v2.rs`). `manifest_version: "2"` identifies the manifest format; `version: "1"` on a capability is its contract version.

The hook can only **tighten** Claude Code's permissions, never relax them: in the worst case it asks too much.
