---
title: HTTP API
description: 本地守护进程的所有路由，附带请求和真实响应。
---

守护进程默认监听 `127.0.0.1:8080`（`server.addr`）。所有请求体都是 JSON。

## 身份验证

如果 `server.token_env` 所指的变量（默认 `JEVALAN_TOKEN`）已设置，则除 `/healthz` 外的所有路由都需要 bearer 令牌：

```sh
curl -s http://127.0.0.1:8080/v1/flows -H "Authorization: Bearer $JEVALAN_TOKEN"
```

`/events` WebSocket 也接受 `?token=<token>`，因为浏览器无法为 WebSocket 设置请求头。

## 路由

| 路由 | 作用 |
|---|---|
| `GET /healthz` | 返回 `ok`。无需身份验证。 |
| `POST /{backend}/v1/systemone` | 把请求体逐字节转发给该后端。 |
| `POST /v1/systemone` | 同上，但发往 `default_backend`。 |
| `POST /v1/decide/{flow}` | 按名称运行一个流程。 |
| `GET /v1/flows` | 列出已加载的流程和无效的流程，并给出原因。 |
| `POST /v1/flows/validate` | 校验一个流程，但不发布。 |
| `POST /v1/flows/draft/run` | 运行一次流程文本，但不发布。 |
| `POST /v1/hub` | 向某个[能力](/zh-cn/capacidades/)请求类型化决策。 |
| `GET /v1/hub/capabilities` | 列出已加载的能力和无效的能力。 |
| `GET /events` | WebSocket，每条消息是一个 JSON 事件。 |

## 运行流程

`state` 可以是任意 JSON 值；你的 CEL 表达式把它读作 `input`。`client` 是可选的，仅用于在 TUI 中标记请求。

```sh
curl -s http://127.0.0.1:8080/v1/decide/ticket-triage \
  -H 'Content-Type: application/json' \
  -d '{"state":{"id":"T-43","priority":"low","topic":"billing"},"client":"docs"}'
```

```json
{"data":{"team":"finance","ticket":"T-43"},"flow":"ticket-triage","outcome":"ok","path":[{"answer":false,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"urgent","port":"false"},{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"billing","port":"true"}],"trace_id":"18e2ca4a68e5b5b40003"}
```

| 字段 | 含义 |
|---|---|
| `outcome` | 流程到达的输出的 `outcome`。 |
| `data` | 输出的 `data`，其中的引用已被解析。 |
| `path` | 访问过的每个节点：它的 `port`、模型的 `answer` 和 `confidence`，以及 `duration_ms`。 |
| `trace_id` | 可以在 TUI 中跟踪的标识符。 |

错误以带有 `error` 消息的 JSON 返回：

| 状态码 | 何时出现 |
|---|---|
| `400` | 请求体中没有 `state`：`{"error":"falta «state»"}`。 |
| `404` | 流程不存在：`{"error":"flujo desconocido: nope"}`。 |
| `502` | 某个节点失败且没有 `error` 端口，或后端不可达。 |

## 校验并试运行草稿

这两个路由都以文本形式接收流程名称及其 YAML，并且都不发布任何内容。你可以在编辑器、CI 任务或脚本中使用它们。

```sh
curl -s http://127.0.0.1:8080/v1/flows/validate \
  -H 'Content-Type: application/json' \
  -d '{"flow":"x","yaml":"description: t\ndefaults: { backend: ollama, model: nimble }\nstart: a\nnodes:\n  a:\n    type: rule\n    when: \"input.x ==\"\n    out: { \"true\": y, \"false\": z }\noutputs:\n  y: { outcome: ok }\n"}'
```

无效的流程会返回 `ok: false` 和确切的问题，包括 CEL 表达式中的位置：

```json
{"error":"a: expresión CEL inválida: ERROR: <input>:1:11: Syntax error: mismatched input '<EOF>' …","ok":false}
```

有效的流程返回 `{"ok": true, "flow": {…}}`。如果还想执行草稿，请添加 `input` 和 `transport`：

```sh
curl -s http://127.0.0.1:8080/v1/flows/draft/run \
  -H 'Content-Type: application/json' \
  -d '{"flow":"x","yaml":"…","input":{"x":1},"transport":"live"}'
```

```json
{"confidence":null,"data":{"r":"yes"},"flow":"x","outcome":"ok","path":[{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"a","port":"true"}],"trace_id":"18e2ca4a96f0d9840007"}
```

## 代理到后端

passthrough 会原样转发你的 System One 请求，并在配置了密钥时添加该后端的 API 密钥。用 `X-Jevalan-Client` 请求头标识你的客户端；默认值是 `unknown`。

```sh
curl -s http://127.0.0.1:8080/ollama/v1/systemone \
  -H 'Content-Type: application/json' \
  -H 'X-Jevalan-Client: my-agent' \
  -d '{"model":"nimble","state":"ls -la","questions":{"q":{"type":"noul","instructions":"Is it destructive?"}}}'
```

如果后端不可达，守护进程会返回 `502`。

## 能力

`POST /v1/hub` 和 `GET /v1/hub/capabilities` 请参见[能力与能力中心](/zh-cn/capacidades/)。

## 事件

`GET /events` 会升级为 WebSocket，每条消息发布一个 JSON 事件：收到的请求、节点的开始与结束、做出的决策、加载的流程和能力，以及错误。终端界面就是这个事件流的客户端。

事件中没有保存原始输入的字段，但可能包含答案、结果、信号、流程定义和错误。请把这个事件流视为敏感数据，在未经检查之前不要分享抓取的内容。
