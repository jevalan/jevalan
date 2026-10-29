---
title: 能力与能力中心
description: 按名称请求类型化决策，无需知道由谁来回答。
---

**能力**是一个有名称、有类型的问题，例如 `tool.risk`（“这次工具调用有风险吗？”）或 `agent.wakeup`（“值得唤醒这个智能体吗？”）。你的智能体向能力中心请求某个能力；由哪个流程、进程或服务来回答，则在配置中决定。你可以更换提供者而无需改动智能体。

## 请求一个决策

```sh
curl -s http://127.0.0.1:8080/v1/hub \
  -H 'Content-Type: application/json' \
  -d '{"protocol_version":"jev-hub/0.1","request_id":"r1","capability":"agent.wakeup","capability_version":"1","input":{"waiting_for":"CI to finish","from_user":true,"skipped":0}}'
```

```json
{"capability":"agent.wakeup","capability_version":"1","decision":{"reason":"user-message","wake":true},"protocol_version":"jev-hub/0.1","provider":{"id":"jevalan-flow","version":"0.1.0"},"request_id":"r1","status":"ok","trace":{"cost":"not_measured","elapsed_ms":2,"path":[{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"user","port":"true"}],"trace_id":"18e2ca4a7cd8c9d00005"}}
```

| 请求字段 | 含义 |
|---|---|
| `protocol_version` | 始终为 `jev-hub/0.1`。 |
| `request_id` | 你自己的标识符；会在响应中原样返回。 |
| `capability`、`capability_version` | 哪个问题，以及它的哪个版本。 |
| `input` | 要判断的数据。必须满足该能力的 `input_schema`。 |
| `constraints.deadline_ms` | 可选。只能缩短该能力的默认截止时间。 |

## 响应

响应始终带有 `status`：

| 状态 | 含义 | 是否带有 `decision` |
|---|---|---|
| `ok` | 提供者做出了决策。 | 是，并且符合 `decision_schema`。 |
| `abstain` | 提供者选择不发表意见。 | 否 |
| `error` | 出了问题，并带有稳定的 `error.code`。 | 否 |

你的智能体应该把 `abstain` 和 `error` 视为“没有答案”，并选择一个安全的默认值；它们从不带有 `decision`。例如，一个未知的能力：

```json
{"capability":"nope.nope","capability_version":"1","error":{"code":"unknown_capability","message":"capacidad desconocida: nope.nope","retryable":false},"protocol_version":"jev-hub/0.1","provider":{"id":"-","version":"-"},"request_id":"r2","status":"error","trace":{"cost":"not_measured","elapsed_ms":0,"trace_id":"18e2ca4a825001080006"}}
```

你还可能看到的错误码有 `invalid_input`（`input` 不符合 `input_schema`）、`version_mismatch`（`capability_version` 与已加载的版本不一致）、`provider_failed`、`deadline_exceeded`、`provider_disabled` 和 `invalid_decision`。`retryable` 告诉你重试是否可能有帮助。

即使是这些错误，HTTP 状态码也是 `200`，因为它们通过 `status` 传递。只有格式错误的请求（例如 `protocol_version` 不是 `jev-hub/0.1`）才会返回 `400`。

## 定义能力

能力是 `hub.capabilities_dir` 中的一个 YAML 文件。文件名就是能力名（`agent.wakeup.yaml`）：

```yaml
manifest_version: "2"
namespace: local.jev
version: "1"
description: "Is it worth waking a sleeping agent?"
input_schema:    { $file: "schemas/agent.wakeup.input.json" }     # JSON Schema，内联或放在文件中
decision_schema: { $file: "schemas/agent.wakeup.decision.json" }
provider: { id: jevalan-flow, flow: agent-wakeup }                 # 由谁来回答
defaults: { deadline_ms: 3000 }
```

两个 schema 都会被强制执行：不符合 `input_schema` 的请求会被拒绝，不符合 `decision_schema` 的提供者响应会变成 `error`。

## 提供者

| 提供者 | 键 | 由谁回答 |
|---|---|---|
| 流程 | `flow` | 你自己的[流程](/zh-cn/flujos/)，在 Jevalan 内部运行。 |
| Http | `http` | 向某个 `url` 发送 POST。 |
| Exec | `exec` | 一个本地进程：`command`，加上 `env` 中列出的环境变量名。 |

`abstain_when` 和 `error_when` 是可选的表达式，作用于提供者的 `response`，可把一次响应转成 `abstain` 或 `error`。这让你无需修改第三方工具就能适配它。如果某个表达式无法求值（例如读取了响应中不存在的字段），响应会是 `error`，错误码为 `provider_failed`。

### 默认禁止运行进程

`exec` 提供者会在你的机器上启动程序，因此除非你设置 `hub.allow_exec: true`（或 `JEVALAN_ALLOW_EXEC=1`），否则它们是禁用的。它们运行时：

- 进程在**没有 shell** 的情况下启动，环境为空，仅有 `PATH`（Windows 上还有 `SystemRoot`）和 `env` 中列出的变量。
- 如果超过截止时间，它会连同整个进程树一起被终止。
- 它的输出上限为 1 MiB。
- 在 Windows 上，`.cmd` 和 `.bat` 文件会被拒绝，因为启动它们要经过 shell。
- 命令、它的环境和它的 URL 永远不会出现在事件、日志或 `GET /v1/hub/capabilities` 中。

当 `allow_exec` 关闭时，`exec` 能力仍然有效，但会返回 `provider_disabled`。

## 查看已加载的内容

```sh
curl -s http://127.0.0.1:8080/v1/hub/capabilities
```

响应会列出每个能力的版本、描述、提供者类型、schema 和默认截止时间，另有一个 `invalid` 对象，说明加载失败的原因。该目录受到监视：保存有效文件会替换能力，无效文件会保留之前的版本，而 `flow` 类型的能力在其流程缺失期间会变为无效。
