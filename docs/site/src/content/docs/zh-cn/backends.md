---
title: 兼容的后端
description: 一个服务器要满足什么条件才能作为 Jevalan 的后端，如何用三个请求检查，以及不满足时会怎样。
---

任何说 [System One](/zh-cn/system-one/) 契约的服务都可以成为 Jevalan 的后端：托管服务、基于开放模型的本地服务器，或模拟器。Jevalan 不区分它们。本页说明对后端有什么要求、接入前如何检查，以及失败时会发生什么。

## 注册后端

后端是[配置](/zh-cn/configuracion/)中 `backends` 下的一项：

```yaml
backends:
  local: { url: "http://127.0.0.1:9000", timeout_ms: 5000 }
default_backend: local
```

流程用 `backend: local` 来指定它；其余配置（密钥、路径）见[配置](/zh-cn/configuracion/)。

## 有什么要求

[流程](/zh-cn/flujos/)按严格规则读取响应。任何违反都会从节点的 `error` 端口出去，而不是让请求失败：

| 要求 | 不满足时 |
|---|---|
| 存在与发送时同名的 `answers.<问题>`。 | `la respuesta no trae la pregunta <名称>`。 |
| 如果答案带有 `type`，必须与节点的一致。 | `respuesta de tipo <x>, se esperaba <y>`。 |
| 带有以类型命名的字段：`noul`、`choice` 或 `score`。 | `falta el campo <类型>`。 |
| `choice` 和 `score` 带有 `confidence`。`noul` 可选：缺失时，Jevalan 根据概率计算。 | `falta confidence`。 |
| `confidence` 在 0 到 1 之间。 | `confianza fuera de [0, 1]`。 |
| `noul` 的值是数字。 | `noul debe ser un número`。 |

此外，如果使用 `batch` 节点，服务器必须接受**一次请求中的多个问题**（`batch` 把所有问题放在一次调用里）；如果流程用到，还必须支持**三种类型**（`noul`、`choice`、`score`）。只实现部分类型的服务器并不算有缺陷，但不受支持类型的每个节点都会从 `error` 出去：见[示例](/zh-cn/ejemplos/)中的第 4 个示例。

## 用三个请求检查

**直接对服务器**发送，不经过 Jevalan，这样才知道问题出在谁那里。替换 URL 和 `<model-id>`：

```sh
URL=http://127.0.0.1:9000/v1/systemone

# noul：返回“是”的概率
curl -s $URL -H 'Content-Type: application/json' -d '{"model":"<model-id>","state":"rm -rf /","questions":{"q":{"type":"noul","instructions":"Is the command destructive?"}}}'

# choice：返回其中一个键，带 probabilities 和 confidence
curl -s $URL -H 'Content-Type: application/json' -d '{"model":"<model-id>","state":"The app crashes on export.","questions":{"q":{"type":"choice","instructions":"What does the user want?","criteria":{"bug":"Something is broken","howto":"A usage question","other":"Anything else"}}}}'

# score：返回刻度上的位置，带 confidence
curl -s $URL -H 'Content-Type: application/json' -d '{"model":"<model-id>","state":"The app crashes on export.","questions":{"q":{"type":"score","instructions":"How urgent is it?","criteria":["none","low","medium","high"]}}}'
```

每个响应都应该与[参考](/zh-cn/system-one/)中的类似：一个带 `type` 和该类型字段的 `answers.q`。对于多个问题的情况，把第一个请求改成两个不同名称的问题，确认两个答案都返回。

三个都正常后，再对 Jevalan 重复（`POST /local/v1/systemone`，见 [HTTP API](/zh-cn/api/)）以检查密钥和截止时间，并运行一个单节点流程查看它的 `path`。如果节点从 `error` 出去，原因在[追踪](/zh-cn/solucion-de-problemas/)里。

## 不匹配时

| 现象 | 可能原因 |
|---|---|
| 某一类型的所有节点都从 `error` 出去。 | 服务器没有实现该类型。 |
| `choice` 或 `score` 报 `falta confidence`。 | 服务器返回了概率，但没有 `confidence` 字段。 |
| `batch` 失败而单个节点正常。 | 服务器每次请求只接受一个问题。 |
| `502` 或超时。 | `url`、`path` 或 `timeout_ms` 有误，或服务器处理你的 `state` 很慢。 |
| 每次运行决策都不同。 | 对模型来说是正常的：在[实验室](/zh-cn/laboratorio/)里用重复次数来衡量。 |

不同的服务器不会产生相同的数字。流程的阈值是针对某个后端调好的：更换后端时，请先用实验室（`jevalan lab live`）重新衡量，再切换到 `enforce`。
