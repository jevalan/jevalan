# AGENTS.md

Operational notes for agents (and people) working in this repository.

## 1. Rust workspace and presentation site

The live product is the Rust workspace in `src/crates/`; `src/sdk-ts/` is reserved as the
reference provider's SDK. Runtime resources live in `config/`. `docs/site/` is a static
presentation and user guide; the workspace serves no web UI or static files. The TUI is the
live product view.

## 2. Orientation

- [`README.md`](README.md) — what jevalan is, what is alive and what is not.
- [`docs/architecture.md`](docs/architecture.md) — how the pieces fit.
- [`docs/local-development.md`](docs/local-development.md) — running it, configuration, and the
  flow editor's keys and manual script.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — the checks a change must pass.
- [`docs/site/`](docs/site/) — static presentation and usage guide.

To confirm your toolchain is healthy, and to see the thing work right now:

```bash
cargo run -p jevalan -- tui --fake
```

`--fake` drives the TUI from a deterministic in-process generator — no daemon, no
capabilities. `--demo` is the richer path (embedded daemon, three mock backends, simulated
traffic).

## 3. Verification

Before declaring a change done, run the same checks CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast --locked
cargo run -p jevalan -- lab check
cargo run -p jevalan -- lab offline
cargo deny check
```

If the build fails with a linker or disk-space error (for example Windows OS error 112), check
free space and keep Cargo's output on a volume with room, with `CARGO_TARGET_DIR`. That is a
storage problem, not a regression.

## 4. Windows: the Git Bash linker trap

Under Git Bash, the `link.exe` shipped with Git shadows the MSVC linker. Scope a clean
`PATH` for the cargo command only:

```bash
PATH="$(dirname "$(command -v cargo)"):/c/Windows/System32:/c/Windows" cargo build
```

Keep that restricted `PATH` for cargo alone: a few tests launch child processes (`.bat` files,
helpers) that need the normal `PATH`, so run the resulting test binaries with it.

## 5. Sandboxed sessions

If the session confines child processes so they may write only under the workspace and a
temporary directory, point Cargo at a writable location, or the build fails with *Access denied*:

```powershell
$env:CARGO_TARGET_DIR = Join-Path $env:TEMP 'jevalan-target'
cargo test --workspace
```

Tests that write through `std::env::temp_dir()` need a writable `TEMP`/`TMP`. A per-command
override pointing inside the repository works:

```powershell
$env:TEMP = Join-Path (Get-Location) '.tmp-tests'; $env:TMP = $env:TEMP
```

An editor's own file tools may succeed where a confined child process (`cargo fmt`) fails. Do
not conclude from a failing child process that the file is unwritable.

## 6. Language

Code comments and the development guides are English; CLI strings and the error messages shown to
users are Spanish. Write new comments in English, and translate a Spanish comment you come across
only when you are already editing that code.

## 7. Editing a flow: `jevalan edit`, and never on the flow that serves

The flow editor is a TUI over one `config/flows/*.yaml`, and it is the only supported way to
change a flow without hand-editing YAML:

```bash
cargo run -p jevalan -- edit config/flows/<flujo>.yaml          # --url/--token/--no-events, see --help
```

What matters before you run it:

- **`config/flows/tool-risk-local.yaml` decides for real.** The Claude Code hook asks it, and
  the daemon hot-reloads the file the moment it changes. Practise on a **copy** under your
  temp directory; the manual script in `docs/local-development.md` does exactly that and checks
  at the end that the original is untouched.
- **So does `config/signals/`.** `tool-risk-local` reads its fixed rules and its four questions
  from there, and the daemon reloads every flow when a signal changes. Editing
  `config/signals/destructive.yaml` changes what the Claude Code hook is asked. Practise on
  copies, with `JEVALAN_SIGNALS_DIR` (daemon) or `--signals-dir` (editor) pointing at them.
- The editor works on a **draft**: it never publishes, never touches the daemon's registry,
  and saving rewrites the YAML **by patches** so comments, order and line endings survive.
- A draft that does not validate is **never written to disk** — not with `s`, not with `g`
  (save aside). If you need to leave a bad draft on disk for inspection, copy it out
  yourself; the editor will refuse.
- With no daemon it validates against the local parser and says in the band that the
  **backends could not be checked**. Everything else — CEL, ports, cycles, models — still is.
- It needs a real terminal: do not launch it from a test or a script. `cargo test -p
  jevalan-edit` covers it headlessly.
