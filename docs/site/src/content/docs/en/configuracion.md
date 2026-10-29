---
title: Configuration
description: Configure the daemon, its backends, and the flow and capability paths.
---

Start with `config.example.yaml`. Copy it to `config.yaml` and keep the configuration together with `config/flows/`, `config/capabilities/`, `config/signals/`, and `config/lab/`.

```yaml
server:
  addr: "127.0.0.1:8080"
backends:
  ollama:
    url: "http://localhost:11434"
default_backend: ollama
flows_dir: ./config/flows
hub:
  capabilities_dir: ./config/capabilities
  signals_dir: ./config/signals
  allow_exec: false
```

Relative paths are resolved from the current working directory. Run Jevalan from the extracted package root so it can find the included resources.

## Settings

Precedence is: command-line flags, then `JEVALAN_*` environment variables, then the file, then defaults. Jevalan also reads a `.env` file in the working directory at startup; it never overrides a variable that is already set.

| Setting | File key | Environment variable | Default |
|---|---|---|---|
| Listen address | `server.addr` | `JEVALAN_ADDR` | `127.0.0.1:8080` |
| Name of the token variable | `server.token_env` | (the variable it names) | `JEVALAN_TOKEN` |
| Backends | `backends.<name>` | see below | only `ollama` |
| Default backend | `default_backend` | `JEVALAN_DEFAULT_BACKEND` | `ollama` |
| Flows directory | `flows_dir` | `JEVALAN_FLOWS_DIR` | `./config/flows` |
| Capabilities directory | `hub.capabilities_dir` | `JEVALAN_CAPABILITIES_DIR` | `./config/capabilities` |
| Signals directory | `hub.signals_dir` | `JEVALAN_SIGNALS_DIR` | `./config/signals` |
| Allow `exec` providers | `hub.allow_exec` | `JEVALAN_ALLOW_EXEC` | `false` |
| Config file | `--config` | `JEVALAN_CONFIG` | none |

## Backends

Each entry under `backends` is a System One service the daemon can forward to:

```yaml
backends:
  typesafe:   { url: "https://api.typesafe.ai", api_key_env: TYPESAFE_API_KEY, timeout_ms: 2000 }
  openrouter: { url: "https://openrouter.ai", path: "/api/alpha/decisions", api_key_env: OPENROUTER_API_KEY, timeout_ms: 8000 }
  ollama:     { url: "http://localhost:11434", timeout_ms: 5000 }
```

| Key | Meaning |
|---|---|
| `url` | Required. It must be `http` or `https`. |
| `api_key_env` | The **name** of the environment variable that holds the key. It is sent as `Authorization: Bearer <key>`. |
| `timeout_ms` | Deadline for one call. Default `5000`. |
| `path` | Path appended to `url`. Default `/v1/systemone`. It must start with `/` and cannot contain `..`. |

Backend names use lowercase letters, digits, and hyphens, and cannot be `v1`. The route your clients call, `/{backend}/v1/systemone`, never changes, whatever the `path` is.

**Ollama as a backend.** From version 0.35.0, Ollama serves decision models over this same contract (for example `ollama pull nimble`) and you call them by model name. With an older version, or without a decision model pulled, `/v1/systemone` does not answer as System One (on 0.32.15 it returns `404`) and every node exits through `error`. An ordinary Ollama chat model will not do: it returns no probabilities of its own.

The daemon fails fast, with a clear message, if `default_backend` is not defined, a `url` is not http or https, or an `api_key_env` variable is empty or unset.

Flows name the backend they use. If a flow names a backend that your configuration does not define, that flow is listed as invalid until you add it. See [Troubleshooting](/en/solucion-de-problemas/).

## Keys and access

Store keys in environment variables and set each variable's name in YAML with `api_key_env`. Do not put real keys in a versioned file.

If you set the token variable (`JEVALAN_TOKEN` by default), every route except `/healthz` requires `Authorization: Bearer <token>`. Use a token whenever other users or processes can reach the daemon's listening address.

Keep `allow_exec: false` unless you explicitly want the hub to launch local processes. See [Capabilities and hub](/en/capacidades/).

See [`config.example.yaml`](https://github.com/jevalan/jevalan/blob/main/config.example.yaml) for the complete structure and example values.

## Watched directories

The daemon watches `flows_dir`, `capabilities_dir`, and `signals_dir`. Saving a valid file replaces its content without a restart. An invalid file keeps the previous version and is reported with its reason. Deleting a file stops serving it.
