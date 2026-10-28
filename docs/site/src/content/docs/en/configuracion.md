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

## Keys and access

Store keys in environment variables and set each variable's name in YAML with `api_key_env`. Do not put real keys in a versioned file. If you configure `token_env`, requests to the daemon must include the token.

Keep `allow_exec: false` unless you explicitly want the hub to launch local processes. Use a token if other users or processes can reach the daemon's listening address.

See [`config.example.yaml`](https://github.com/jevalan/jevalan/blob/main/config.example.yaml) for the complete structure and example values.
