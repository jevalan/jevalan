---
title: Troubleshooting
description: Fix the most common problems with the daemon, flows, hooks, and the lab.
---

## A flow or capability is "invalid"

At startup the daemon prints one line per flow and capability, and `GET /v1/flows` and `GET /v1/hub/capabilities` list the invalid ones under `invalid`, with the reason:

```text
flujo cargado: ticket-triage (2 nodos)
flujo inválido: agent-wakeup: worth: backend desconocido "openrouter"
capacidad inválida: agent.wakeup: proveedor inválido: flujo desconocido: agent-wakeup
```

The usual cause is a flow that names a backend your configuration does not define (`backend desconocido`). Add the backend to `backends:` in your config, with its key if it needs one. A capability served by a flow becomes invalid while that flow is invalid, so fixing the flow fixes both.

This is expected with the minimal configurations, which define only some backends. It is not a fault, but the flows that need the missing backends will not be served.

## The daemon does not start

- **A variable is missing.** The daemon fails fast if `default_backend` is not defined, a `url` is not http or https, or an `api_key_env` names a variable that is empty or unset. The message says which one.
- **The port is taken.** Change `server.addr`, or set `JEVALAN_ADDR`.
- **Relative paths.** `flows_dir`, `capabilities_dir`, and `signals_dir` are resolved from the folder where you start the daemon. Start it from the project root, or use absolute paths and set `signals_dir` as well, or `tool-risk-local` will not load.

## The terminal UI shows nothing

The TUI is a viewer: activity appears only when a client sends requests. Check, in this order:

1. Is the daemon running? `curl http://127.0.0.1:8080/healthz` should print `ok`.
2. Does the daemon require a token? Pass `--token` or set `JEVALAN_TOKEN`. When it is not passed, the TUI takes the token from the discovery file.
3. Want to see it working without anything else? Run `jevalan tui --demo`, which brings its own services and traffic.

## Requests get `502`

The backend is unreachable, or a node failed and has no `error` port. Check the backend URL and key, and add an `error` port to the node so the flow can handle the failure on purpose instead of ending the request.

## A hook does nothing

That is what `shadow` mode is for: it prints nothing and always exits 0. To check that it works:

- Look at the shadow log (`shadow.jsonl`, in the data directory) or press `H` in the TUI: calls should appear there.
- If nothing is recorded, the daemon is probably not running. The hook reads `daemon.json` to find it and, without it, does not try to connect.
- A `daemon.json` left over from a daemon that was killed (instead of stopped normally) points to a port that no longer answers. Start the daemon again, or delete the file.
- In `enforce` mode, anything the hub cannot decide (no daemon, abstain, error, timeout) answers `ask`, so you will be asked more often while the daemon is down.

## `exec` capabilities answer `provider_disabled`

Running processes is off by default. Set `hub.allow_exec: true` (or `JEVALAN_ALLOW_EXEC=1`) if you trust the capability. If a variable such as `${TOOLGATE_CLI}` is used in the command and is not defined, the capability becomes invalid when `allow_exec` is on.

## `lab offline` fails

- **"no fixture was run"** means every fixture was filtered or skipped. Check `modes`, and whether the cassettes are stale.
- **A fixture is `skipped` for a stale cassette.** You changed the fixture's `version` or `input`. Run `jevalan lab record` again for it.
- **A `replay` key is rejected by `lab check`.** The key must be a node of the flow or the provider's `id`.

## Building from source

- **Windows with Git Bash:** the `link.exe` that ships with Git hides the MSVC linker and the build fails with a linker error. Run Cargo with a clean `PATH` for that command only: `PATH="$(dirname "$(command -v cargo)"):/c/Windows/System32:/c/Windows" cargo build`.
- **Out of disk space** (Windows error 112): keep Cargo's output on a volume with room, with `CARGO_TARGET_DIR`.
- **Rust version:** you need Rust 1.89 or later.

Still stuck? [Open an issue](https://github.com/jevalan/jevalan/issues) with the command you ran and the daemon's output, and remove any keys first. For a security problem, follow the [security policy](https://github.com/jevalan/jevalan/blob/main/SECURITY.md) instead.
