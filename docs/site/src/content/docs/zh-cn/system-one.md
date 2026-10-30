---
title: System One 参考
description: System One 的契约：Jevalan 转发、其流程所使用的类型化决策 API，并附可下载的 OpenAPI 文件。
---

System One 是一个快速的类型化决策 API：你发送一个 `state` 和最多 64 个有名称的问题，它为每个问题返回一个带类型、带置信度的答案。Jevalan 原样转发（passthrough），[流程](/zh-cn/flujos/)中的 `noul`、`choice` 和 `score` 节点底层说的就是它。

:::caution[不是官方规范]
本页描述的是 **Jevalan 所观察到的契约**：它的代码、它的测试服务器，以及暴露同一端点的网关的公开文档。这些来源没有明确规定的内容（错误响应体、大小限制）这里不会臆造。如有疑问，以你的提供者的文档为准。
:::

下载本契约的 OpenAPI 3.1 描述：[`system-one.openapi.yaml`](/system-one.openapi.yaml)。可用于生成客户端、校验请求，或在任何 OpenAPI 查看器中打开。

## 端点

`POST /v1/systemone`，请求体和响应均为 JSON，带 `Authorization: Bearer <密钥>`。通过 Jevalan：

| 路由 | 到达哪里 |
|---|---|
| `POST /v1/systemone` | 默认后端（`default_backend`）。 |
| `POST /{backend}/v1/systemone` | 你指定的后端。 |

Jevalan 逐字节转发请求体，如果配置了后端密钥就加上，并原样返回后端的状态和响应体。如果后端没有响应，则返回 `502`。详情见 [HTTP API](/zh-cn/api/) 和[配置](/zh-cn/configuracion/)。

## 请求

| 字段 | 类型 | 含义 |
|---|---|---|
| `model` | 文本 | 该后端上的模型标识符。 |
| `state` | 文本 | 被判断的内容。 |
| `questions` | 对象 | 有名称的问题：1 到 64 个。 |

每个问题带有一个 `type`，并根据类型而不同：

| `type` | `instructions` | `criteria` | 回答 |
|---|---|---|---|
| `noul` | 问题本身，表述成是或否。 | 可选：仅有 `"true"` 和 `"false"` 两个键，各说明其含义。 | “是”的概率。 |
| `choice` | 问题本身。 | 必填：每个选项的 键 → 描述 映射。 | 其中一个键。 |
| `score` | 问题本身。 | 必填：等级列表，由低到高。 | 该刻度上的一个位置。 |

```json
{
  "model": "<model-id>",
  "state": "I was charged twice for my subscription.",
  "questions": {
    "refund": { "type": "noul", "instructions": "Is the customer asking for money back?" },
    "department": {
      "type": "choice",
      "instructions": "Which team should handle this?",
      "criteria": { "billing": "Charges and refunds", "technical": "Bugs and outages" }
    },
    "urgency": { "type": "score", "instructions": "How urgent is it?", "criteria": ["none", "low", "medium", "high"] }
  }
}
```

## 响应

`answers` 中每个问题对应一项，名称相同，另有本次调用的 `usage`：

| 类型 | 响应字段 |
|---|---|
| `noul` | `noul`（“是”的概率，0 到 1）和 `confidence`。 |
| `choice` | `choice`（选中的键）、`probabilities`（每个键一个）和 `confidence`。 |
| `score` | `score`（刻度上的位置，从 0 到最后一个等级的索引）和 `confidence`。 |

```json
{
  "model": "<model-id>",
  "answers": {
    "refund": { "type": "noul", "noul": 0.98, "confidence": 0.9 },
    "department": { "type": "choice", "choice": "billing", "probabilities": { "billing": 0.91, "technical": 0.09 }, "confidence": 0.88 },
    "urgency": { "type": "score", "score": 2.1, "confidence": 0.74 }
  },
  "usage": { "input_tokens": 275, "output_tokens": 20 }
}
```

示例中的数字仅为示例值。响应不是流式的：一次性完整到达。

## 流程如何使用它

[流程](/zh-cn/flujos/)的一个节点就是一个带输出端口的 System One 问题：

| 节点 | 问题 | 输出端口 |
|---|---|---|
| `noul` | `type: noul` | 按 `threshold` 分为 `"true"` / `"false"`。 |
| `choice` | `type: choice`，带 `criteria` | `criteria` 中每个键一个。 |
| `score` | `type: score`，带 `criteria` | 刻度上的 `a..b` 区间。 |
| `batch` | 一次调用中的多个问题 | `route` 规则能看到所有答案。 |

此外，调用模型的节点可以连接 `error` 端口（后端失败或回答有误）和 `low-confidence` 端口（`confidence` 低于 `min_confidence`；不连接则忽略置信度，`batch` 不支持该端口）。因此，不支持某种问题类型的后端会让每个请求都从 `error` 端口出去：见[示例](/zh-cn/ejemplos/)中的第 4 个示例。

## 错误

公开的来源只明确了少数情况：既没有 `instructions` 也没有 `criteria` 的 `noul` 问题返回 `400`，密钥错误返回 `401`，被网关拒绝访问返回 `403`。错误响应体没有统一的 schema，因此请把任何非 `200` 的状态视为“没有答案”，并按[决策指南](/zh-cn/guia-decisiones/)所述选择默认值。
