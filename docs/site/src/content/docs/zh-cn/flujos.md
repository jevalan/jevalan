---
title: 流程
description: 用 YAML 编写决策流程，组合规则、模型提问和类型化输出。
---

**流程**是用 YAML 编写的小型决策图。它从一个节点开始，沿着端口在节点之间移动，最后到达一个**输出**，输出带有 `outcome`，也可以带有 `data`。流程就是 `flows_dir`（默认 `./config/flows`）中的普通文件，所以你可以在 pull request 中评审它们，并纳入版本控制。

## 流程的结构

```yaml
description: "Route a support ticket"      # 显示在 TUI 和 GET /v1/flows 中
defaults: { backend: ollama, model: nimble }  # 向模型提问的节点会使用它
start: urgent                              # 第一个节点
nodes:
  urgent:
    type: rule                             # 不涉及模型
    when: 'input.priority == "high"'       # 针对请求的 CEL 表达式
    out: { "true": page, "false": billing }  # 端口 -> 下一个节点或输出
  billing:
    type: rule
    when: 'input.topic == "billing"'
    out: { "true": finance, "false": queue }
outputs:
  page:    { outcome: ok, data: { team: oncall,  ticket: "$input/id" } }
  finance: { outcome: ok, data: { team: finance, ticket: "$input/id" } }
  queue:   { outcome: ok, data: { team: support, ticket: "$input/id" } }
```

把它保存为 `config/flows/ticket-triage.yaml`，守护进程无需重启就会加载它。向它请求一个决策：

```sh
curl -s http://127.0.0.1:8080/v1/decide/ticket-triage \
  -H 'Content-Type: application/json' \
  -d '{"state":{"id":"T-42","priority":"high","topic":"login"}}'
```

```json
{"data":{"team":"oncall","ticket":"T-42"},"flow":"ticket-triage","outcome":"ok","path":[{"answer":true,"backend":"-","confidence":1.0,"duration_ms":0,"model":"-","node":"urgent","port":"true"}],"trace_id":"18e2ca4a6312005c0002"}
```

请求中的 `state` 就是你的表达式所看到的 `input`。响应会告诉你 `outcome`、输出的 `data`，以及流程走过的 `path`（每个节点一条记录），所以你总能看清一个决策为什么是这个结果。

## 节点类型

| 类型 | 作用 | 是否调用模型 |
|---|---|---|
| `rule` | 针对 `input` 计算一个 [CEL](https://cel.dev) 表达式。端口：`true` 和 `false`。 | 否 |
| `noul` | 提出一个是/否问题并得到一个概率。可以用 `threshold` 划分，也可以把概率区间映射到端口。 | 是 |
| `choice` | 让模型从 `criteria` 中选出一个标签。每个标签就是一个端口。 | 是 |
| `score` | 让模型针对一组有序的 `criteria` 打分。端口是数值区间，例如 `"0..1.5"`。 | 是 |
| `batch` | 在**一次**后端调用中提出多个问题，并根据所有答案进行路由。 | 是（一次调用） |

数值区间包含其下限：`score` 为 `1.5` 时会走端口 `"1.5..3"`。

向模型提问的节点带有 `instructions`（问题）和 `criteria`，并且除非自己指定，否则从 `defaults` 继承 `backend` 和 `model`。

除了你自己的端口，带模型的节点还可以使用两个特殊端口：

- `error`：后端失败或超时时进入。没有它时，失败的节点会以 `502` 结束请求。
- `low-confidence`：模型的置信度低于 `min_confidence` 时进入。

下面这个按难度把任务路由给模型的流程来自仓库中的 `config/flows/model-route.yaml`，它用到了以上所有内容：

```yaml
nodes:
  tier:
    type: score
    instructions: "How difficult is the task?"
    criteria: ["trivial", "easy", "medium", "hard", "very hard"]
    out: { "0..1.5": small, "1.5..3": risky, "3..4": large, error: medium, low-confidence: medium }
  risky:
    type: noul
    instructions: "Does the task touch security, production data, or money?"
    out: { "true": large, "false": medium }
outputs:
  small:  { outcome: route, data: { model: haiku, effort: low } }
  medium: { outcome: route, data: { model: sonnet, effort: medium } }
  large:  { outcome: route, data: { model: opus, effort: high } }
```

## 输出与引用

输出有一个 `outcome`（你可以自定义标签：`ok`、`allow`、`deny`、`route`……）和一个可选的 `data` 对象，它会返回给调用方。在 `data` 中，你可以用引用来复制值：

| 引用 | 值 |
|---|---|
| `"$input/id"` | 请求 `state` 中的 `id` 字段。 |
| `"$answer/<node>"` | 某个模型节点的答案。 |
| `"$answer/<node>/<question>"` | `batch` 节点的某一个答案。 |

## Batch：多个问题，一次调用

`batch` 节点可以避免串联多次调用，并让你根据所有答案一起做决定。答案可以通过 `answers.<node>.<question>` 访问：

```yaml
risks:
  type: batch
  questions:
    destructive:  { type: noul, instructions: "…", criteria: { "true": "…", "false": "…" } }
    exfiltration: { type: noul, instructions: "…", criteria: { "true": "…", "false": "…" } }
  route:                                   # 按顺序：第一个为真的 `when` 生效
    - { when: "answers.risks.destructive >= 0.85", to: deny }
    - { when: "answers.risks.destructive >= 0.55", to: ask }
  else: allow                              # 没有任何 `when` 匹配时
  out: { error: ask_error }                # 可选：后端失败时去哪里
```

## 可复用的信号

当多个流程提出同一个问题时，把它在 `config/signals/<name>.yaml` 中声明一次作为**信号**，然后在流程中引用它的名字。信号是一个没有接线的节点主体：包含 `type`、`instructions`、`criteria` 或 `when`，但绝不包含 `out`、`backend` 或 `model`。文件名就是信号名。

- **启发式**信号（`type: rule`）只针对 `input` 使用 CEL。任何 `when` 都通过 `signals.<name>` 读取它。
- **学习型**信号（`noul`、`choice`、`score`）是一个问题。`batch` 用 `signals: [name, …]` 来提问。

自带的 `config/flows/tool-risk-local.yaml` 把两者结合起来：先运行两条固定规则，再用一次调用向模型提出四个问题。

```yaml
# config/signals/rm_root.yaml — 一个启发式信号
type: rule
when: 'has(input.tool_input.command) && input.tool_input.command.matches(r"rm\s+-[a-zA-Z]*[rf][a-zA-Z]*\s+(/|~|\$HOME)(\s|$)")'
```

```yaml
# 在流程中
risks:
  type: batch
  signals: [destructive, exfiltration, privilege, secret_exposure]
  route:
    - { when: 'answers.risks.exists(k, answers.risks[k] >= 0.85)', to: deny }
    - { when: 'answers.risks.exists(k, answers.risks[k] >= 0.55)', to: ask }
  else: allow
```

启发式信号一旦失败，会把它的节点送到 `error` 端口；它绝不会被当作 `false`。

## 安全地编辑流程

- **热重载。** 守护进程会监视 `flows_dir` 和 `signals_dir`。保存有效的文件会替换流程；无效的文件会保留之前的版本，并在 `GET /v1/flows` 中列出原因。
- **保存前先校验。** 把 YAML 发送到 `POST /v1/flows/validate`，或用 `POST /v1/flows/draft/run` 运行一次草稿。两者都不会发布任何内容。参见 [HTTP API](/zh-cn/api/)。
- **使用可视化编辑器。** `jevalan edit config/flows/<flow>.yaml` 会在终端中打开草稿，并以补丁方式写回修改，所以注释和顺序都会保留。它绝不会写入未通过校验的草稿。
- **留意线上流程。** 安装了 Claude Code hook 之后，`config/flows/tool-risk-local.yaml` 和 `config/signals/` 是真正在做决策的。请在副本上练习。
