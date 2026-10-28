# Installing the Claude Code hook (Windows)

*[Versión en español](es/claude-code-hook-install.md)*

How the `PreToolUse` hook is set up in shadow mode, and how to update it. The design is in `docs/architecture.md`, section 6.

## Installed pieces

| Piece | Where |
|---|---|
| Stable binary (used by the hook and the daemon) | `%LOCALAPPDATA%\jevalan\bin\jevalan.exe` |
| Global hook | `~/.claude/settings.json` → `PreToolUse`, matcher `*`, `jevalan.exe hook claude-code --mode shadow` |
| Daemon | scheduled task `jevalan-daemon` (at logon), cwd = repo, `serve --config config/lab/config.openrouter.yaml` |
| Daemon output | `%LOCALAPPDATA%\jevalan\serve.log` |
| Discovery (addr + token) | `%LOCALAPPDATA%\jevalan\daemon.json` |
| Shadow log | `%LOCALAPPDATA%\jevalan\shadow.jsonl` |

The hook points to a **copy** of the binary, never to `target/`: a `cargo clean` or a half-finished build must not break every Claude Code session.

The TUI takes the token from `daemon.json` when it is not given `--token` and there is neither `JEVALAN_TOKEN` nor a `.env`, so `jevalan tui` works from any folder.

## Running `jevalan` from any terminal

The stable binary's directory is not on the `PATH` by default. To be able to type plain `jevalan`, add it to the **user** `PATH` (no admin needed, and this way the console and the daemon run the same copy):

```powershell
$dir = Join-Path $env:LOCALAPPDATA 'jevalan\bin'
$cur = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($cur -split ';') -contains $dir) {
    "Already on the user PATH."
} else {
    [Environment]::SetEnvironmentVariable('Path', ($cur.TrimEnd(';') + ';' + $dir), 'User')
    "Added: $dir"
}
```

Two details that matter:

- **It reads the user variable, not `$env:PATH`.** `$env:PATH` is the merged view of user and machine; writing it back to the user variable would embed the machine paths inside yours. The prior check avoids duplicating the entry if you run it twice.
- **It only affects new processes.** A terminal that is already open keeps the `PATH` it started with, so `Get-Command jevalan` fails there even though the change is done. Open another one, or refresh the current one:

  ```powershell
  $env:PATH = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')
  ```

Check in a new terminal:

```powershell
Get-Command jevalan
jevalan --version
```

The binary answers to `--version`.

The hook and the scheduled task keep invoking the **absolute path**, so they are not affected by this change.

Because what ends up on the `PATH` is the **stable** copy, `jevalan` runs the installed version, which may lag behind the source — that copy exists precisely so that a `cargo clean` or a half-finished build does not break every Claude Code session. For the current source, `cargo run -p jevalan -- tui`; to bring the copy up to date, `cargo build --release -p jevalan` and `./tools/scripts/update-installed-binary.ps1`.

## Replacing the .exe after rebuilding

```powershell
cargo build --release -p jevalan
./tools/scripts/update-installed-binary.ps1
```

You must rebuild and reinstall the stable copy whenever `config/flows/` uses something the installed binary does not know — for example `signals:`, since the signal layer. An older binary keeps the version of the flow it already had loaded, but after a restart it marks that flow as invalid and stops serving it.

### If it fails with "the process cannot access the file because it is being used by another process"

Windows does not let you overwrite a running `.exe`, and the stable copy is used by **the daemon** (always) and **the hook** (for an instant on every tool). The script already handles it; by hand it is:

```powershell
Stop-ScheduledTask jevalan-daemon                 # 1. stop the daemon
Get-Process jevalan | Select-Object Id, Path      # 2. any left? (e.g. a serve launched by hand)
Copy-Item target\release\jevalan.exe "$env:LOCALAPPDATA\jevalan\bin\jevalan.exe" -Force
Start-ScheduledTask jevalan-daemon                # 3. start it again
```

If the copy still fails, it is a hook in flight: wait a second and repeat. While the daemon is stopped, the hook in shadow mode does not get in the way: it records `"sin daemon"` (no daemon) and exits.

## Managing the daemon

```powershell
Start-ScheduledTask jevalan-daemon
Stop-ScheduledTask jevalan-daemon
Get-Content "$env:LOCALAPPDATA\jevalan\serve.log" -Tail 20
Unregister-ScheduledTask jevalan-daemon -Confirm:$false   # remove it entirely
```

## Uninstalling the hook

Remove the `jevalan.exe` entry from `PreToolUse` in `~/.claude/settings.json` (or disable it from `/hooks`).

## Privacy: local models only

The hook is **global**: every tool of **any** project on the machine (including client repos and personal repos with tokens, `.env` files, Kubernetes secrets…) is sent to the daemon, and the daemon passes it to whatever serves `tool.risk`. Shadow-log redaction only protects the local file; **what the daemon forwards goes out as is**.

Therefore:

- The hook's daemon uses **`config/lab/config.local.yaml`**: `tool.risk` is decided by `config/flows/tool-risk-local.yaml` against **Kev (`127.0.0.1:8009`) or Laya (`127.0.0.1:8021`)**. Nothing leaves the machine.
- It is **forbidden** to point that daemon at a cloud backend or specialist (Toolgate, Jev via OpenRouter or TypeSafe) until the outbound shield exists:
  1. do not query at all if the tool touches sensitive paths (`.env*`, `*.pem`, `*.key`, `id_rsa*`, kubeconfig, `secret*.yaml`, `sealed*`, `credentials*`, `.npmrc`…), including commands that name them;
  2. redact the **outgoing input** too, not just the log, leaving `[REDACTED:<type>]` marks;
  3. do not pass `transcript_path` as is: generate a sanitized temporary transcript;
  4. a folder allowlist (`cwd`);
  5. redaction patterns validated against a secret scan of your own repositories (type and file, never values).
- `shadow.jsonl` is local but stores the full commands (with what is recognizable redacted): it is not uploaded to any repo or shared. What is publishable is `jevalan shadow report`, which includes no inputs.
