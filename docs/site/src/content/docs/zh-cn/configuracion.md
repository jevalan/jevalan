---
title: 配置
description: 配置守护进程、后端，以及流程和能力的路径。
---

从 `config.example.yaml` 开始。把它复制为 `config.yaml`，并让配置与 `config/flows/`、`config/capabilities/`、`config/signals/` 和 `config/lab/` 放在一起。

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

相对路径从当前工作目录开始解析。请在解压后的软件包根目录中运行 Jevalan，以便它能找到附带的资源。

## 设置项

优先级为：命令行参数，其次是 `JEVALAN_*` 环境变量，再次是配置文件，最后是默认值。Jevalan 启动时还会读取工作目录中的 `.env` 文件；它绝不会覆盖已经设置的变量。

| 设置 | 文件中的键 | 环境变量 | 默认值 |
|---|---|---|---|
| 监听地址 | `server.addr` | `JEVALAN_ADDR` | `127.0.0.1:8080` |
| 令牌变量的名称 | `server.token_env` | （它所指的变量） | `JEVALAN_TOKEN` |
| 后端 | `backends.<name>` | 见下文 | 仅 `ollama` |
| 默认后端 | `default_backend` | `JEVALAN_DEFAULT_BACKEND` | `ollama` |
| 流程目录 | `flows_dir` | `JEVALAN_FLOWS_DIR` | `./config/flows` |
| 能力目录 | `hub.capabilities_dir` | `JEVALAN_CAPABILITIES_DIR` | `./config/capabilities` |
| 信号目录 | `hub.signals_dir` | `JEVALAN_SIGNALS_DIR` | `./config/signals` |
| 允许 `exec` 提供者 | `hub.allow_exec` | `JEVALAN_ALLOW_EXEC` | `false` |
| 配置文件 | `--config` | `JEVALAN_CONFIG` | 无 |

## 后端

`backends` 下的每一项都是守护进程可以转发到的 System One 服务：

```yaml
backends:
  typesafe:   { url: "https://api.typesafe.ai", api_key_env: TYPESAFE_API_KEY, timeout_ms: 2000 }
  openrouter: { url: "https://openrouter.ai", path: "/api/alpha/decisions", api_key_env: OPENROUTER_API_KEY, timeout_ms: 8000 }
  ollama:     { url: "http://localhost:11434", timeout_ms: 5000 }
```

| 键 | 含义 |
|---|---|
| `url` | 必填。必须是 `http` 或 `https`。 |
| `api_key_env` | 保存密钥的环境变量的**名称**。它以 `Authorization: Bearer <key>` 的形式发送。 |
| `timeout_ms` | 单次调用的截止时间。默认 `5000`。 |
| `path` | 追加到 `url` 后面的路径。默认 `/v1/systemone`。必须以 `/` 开头，且不能包含 `..`。 |

后端名称使用小写字母、数字和连字符，且不能是 `v1`。无论 `path` 是什么，你的客户端调用的路由 `/{backend}/v1/systemone` 都不会改变。

**把 Ollama 当作后端。** 从 0.35.0 版本起，Ollama 通过同一契约提供决策模型（例如 `ollama pull nimble`），按模型名调用。版本更旧，或没有拉取决策模型时，`/v1/systemone` 不会按 System One 应答（0.32.15 返回 `404`），所有节点都会走 `error` 端口。普通的 Ollama 聊天模型不行：它不会返回自己的概率。

如果 `default_backend` 未定义、某个 `url` 不是 http 或 https，或者某个 `api_key_env` 变量为空或未设置，守护进程会立即失败，并给出清晰的信息。

流程会指定它们所用的后端。如果流程指定了你的配置中没有定义的后端，该流程会显示为无效，直到你把它加上。参见[故障排查](/zh-cn/solucion-de-problemas/)。

## 密钥与访问

把密钥存放在环境变量中，并在 YAML 里用 `api_key_env` 写上每个变量的名称。不要把真实密钥放进受版本控制的文件。

如果你设置了令牌变量（默认 `JEVALAN_TOKEN`），则除 `/healthz` 外的所有路由都需要 `Authorization: Bearer <token>`。只要其他用户或进程能够访问守护进程的监听地址，就请使用令牌。

除非你明确希望能力中心启动本地进程，否则请保持 `allow_exec: false`。参见[能力与能力中心](/zh-cn/capacidades/)。

完整的结构和示例值请参阅 [`config.example.yaml`](https://github.com/jevalan/jevalan/blob/main/config.example.yaml)。

## 被监视的目录

守护进程会监视 `flows_dir`、`capabilities_dir` 和 `signals_dir`。保存有效文件会在不重启的情况下替换其内容。无效文件会保留之前的版本，并报告原因。删除文件则停止提供该内容。
