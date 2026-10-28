# Installing the Crush hook (Windows)

*[Versión en español](es/crush-hook-install.md)*

How to set up the `PreToolUse` hook so Crush goes through jevalan, in shadow mode and
never slowing it down. It plays the same role as the Claude Code hook (`docs/claude-code-hook-install.md`),
but through its own subcommand `jevalan hook crush`: it signs each request as `crush/<project>:<session>`
— so it does not show up as `unknown` in the TUI — and answers in Crush's native envelope.

## Why it fits without a bridge

Crush hooks are compatible with Claude Code's, and their contract already matches what
jevalan needs:

- **Input (stdin JSON):** `{event, session_id, cwd, tool_name, tool_input}` — exactly the fields
  that `extract_input` reads.
- **Output:** Crush understands `{"decision":"deny","reason":"…"}`, which is what `enforce` mode
  emits. Crush only tells `allow` from `deny`; it cannot ask for confirmation from a hook,
  so in Crush active mode **blocks or lets its permissions decide**, and never forces a
  dialog. The hook never emits `allow`: it can only tighten.

## Pieces

| Piece | Where |
|---|---|
| Stable binary | `%LOCALAPPDATA%\jevalan\bin\jevalan.exe` |
| Global hook | `%USERPROFILE%\.config\crush\crushrc` → `hook add PreToolUse …` |
| Daemon | scheduled task `jevalan-daemon` (at logon) |
| Daemon output | `%LOCALAPPDATA%\jevalan\serve.log` |
| Discovery (addr + token) | `%LOCALAPPDATA%\jevalan\daemon.json` |
| Shadow log | `%LOCALAPPDATA%\jevalan\shadow.jsonl` |

The hook points to a **copy** of the binary, never to `target/`: a `cargo clean` or a half-finished
build must not break every Crush session. To bring it up to date, `cargo build --release -p jevalan`
and `./tools/scripts/update-installed-binary.ps1`.

## Configuring the hook

In the global `crushrc` (`%USERPROFILE%\.config\crush\crushrc`), one line:

```bash
hook add PreToolUse --name jevalan --matcher '^(bash|edit|write|multiedit)$' --command '"$LOCALAPPDATA/jevalan/bin/jevalan.exe" hook run --client crush --dialect crush --mode shadow' --timeout 8
```

- The subcommand is generic: `hook run --client <name> --dialect <claude|crush>`. Adding a new
  harness adds no code to jevalan, only changes flags; `hook crush` is an alias for
  `hook run --client crush --dialect crush`.
- The `--matcher` leaves out the tools that neither execute anything nor touch the disk (`view`, `ls`,
  `grep`, `glob`, `question`, `todos`, …): the Rust hook queries for every tool with a `tool_name`,
  so the filter lives here, not in jevalan.
- `--command` is a shell line that Crush runs with its embedded shell; `$LOCALAPPDATA` is
  expanded when it is launched. If you prefer an absolute path or `crush.json`, the equivalent is:

  ```jsonc
  {
    "hooks": {
      "PreToolUse": [
        {
          "name": "jevalan",
          "matcher": "^(bash|edit|write|multiedit)$",
          "command": "\"%LOCALAPPDATA%\\jevalan\\bin\\jevalan.exe\" hook run --client crush --dialect crush --mode shadow",
          "timeout": 8
        }
      ]
    }
  }
  ```

Change `--mode shadow` to `--mode enforce` so that a `deny` verdict blocks the tool.
In `enforce`, `ask` and indecision let it through: the hook cannot ask for confirmation.

Hooks are re-read when Crush starts; to apply the change, restart the session.

## Checking

With the daemon up, a tool (for example `bash`) should appear in the TUI as
`crush/<project>:<session>`, and every decision is recorded in `shadow.jsonl`. Without a daemon, the hook
records `"sin daemon"` (no daemon) and exits without getting in the way.

## Uninstalling

Remove the `jevalan` entry from the `crushrc` (`hook remove PreToolUse --name jevalan`) or from the
`hooks.PreToolUse` block of `crush.json`.

## Privacy: local models only

The hook is **global**: every tool of **any** project on the machine is sent to the daemon.
So the same rule applies as for the Claude Code hook (see `docs/local-development.md`):

- The hook's daemon uses **`config/lab/config.local.yaml`**: `tool.risk` is decided by `config/flows/tool-risk-local.yaml`
  against **Kev (`127.0.0.1:8009`) or Laya (`127.0.0.1:8021`)**. Nothing leaves the machine.
- It is **forbidden** to point that daemon at a cloud backend or specialist until the outbound
  shield exists.
- `shadow.jsonl` stores the full commands (with what is recognizable redacted): it is not uploaded to any
  repo or shared. What is publishable is `jevalan shadow report`.
