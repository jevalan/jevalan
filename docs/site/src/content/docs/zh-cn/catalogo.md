---
title: 能力目录
description: Jevalan 自带哪些能力、每个能力回答什么问题、由谁使用，以及没有答案时该怎么办。
---

本页是清单。关于机制（如何请求能力、如何定义、响应有哪些状态），请看[能力与能力中心](/zh-cn/capacidades/)。这里列的是现有的和尚不存在的。

每个能力都带有以下状态之一：

| 状态 | 含义 |
|---|---|
| **已包含** | 仓库中有清单、schema、流程或提供者，以及[实验室](/zh-cn/laboratorio/)的 fixture。 |
| **示例流程** | 以流程形式存在（`POST /v1/decide/{流程}`），但没有在能力中心注册为能力。 |
| **研究中** | 只是想法，未实现，也没有日期。 |

## `tool.risk`（已包含）

**问题：**这次工具调用有风险吗？回答 `allow`、`ask` 或 `deny`。

| | |
|---|---|
| 输入 | `tool_name` 和 `tool_input`（必填）；`cwd`、`session_id`、`tool_use_id` 和 `transcript_path`（可选）。 |
| 决策 | `verdict`（`allow`、`ask`、`deny`）、`reason`、`source`，以及用过模型时的 `probabilities`。 |
| 截止时间 | 默认 5000 毫秒。 |
| 由谁提供 | `config/capabilities-local/tool.risk.yaml` 用 `tool-risk-local` 流程在本地完成决策。`config/capabilities/tool.risk.yaml` 通过 `exec` 提供者交给外部工具，需要 `hub.allow_exec: true`。 |
| 由谁使用 | Claude Code 和 Crush 的[钩子](/zh-cn/integraciones/)，以及 [MCP 服务器](/zh-cn/servidor-mcp/)的 `tool_risk` 工具。 |
| 没有答案 | 钩子回答 `ask`，绝不盲目 `allow`。 |

本地流程先应用两条固定规则（递归删除根目录或主目录，以及把下载内容通过管道交给解释器），只有都不触发时，才在一次调用里向模型问四件事：破坏、外泄、提权和泄露机密。风险达到 0.85 或以上为 `deny`，达到 0.55 或以上为 `ask`，更低则为 `allow`。规则和这四个问题放在 `config/signals/`，任何流程都可以复用。

## `agent.wakeup`（已包含）

**问题：**智能体在休眠中等待某件事，现在来了一个事件：值得唤醒它吗？

| | |
|---|---|
| 输入 | `waiting_for` 和 `skipped`（必填）；`event`、`observation`、`from_user` 和 `max_skips`（可选）。 |
| 决策 | `wake`（布尔值）、`reason`，以及用过模型时的 `probability`。 |
| 截止时间 | 默认 3000 毫秒。 |
| 由谁提供 | `agent-wakeup` 流程，通过 `config/capabilities/agent.wakeup.yaml`。 |
| 由谁使用 | 对 `POST /v1/hub` 的调用，以及 MCP 服务器的 `agent_wakeup` 工具。 |
| 没有答案 | 模型失败时，流程自己会唤醒智能体（`reason: error`）：宁可多唤醒一次，也不让智能体永远沉睡。 |

可能的 `reason` 有 `user-message`、`nothing-to-judge`、`skip-limit`、`judged`、`unsure` 和 `error`。前三个是确定性规则，不会咨询任何模型：用户消息总会唤醒；没有事件也没有观察的通知同样唤醒；连续跳过 `max_skips` 次（默认 10）后也会唤醒。只有其余情况才会交给模型。

## `agent.stop`（已包含）

**问题：**智能体说它做完了：它的工作可以视为完成吗？这是“完成关卡”：在让智能体停下之前先要求证据。

| | |
|---|---|
| 输入 | `task` 和 `blocked`（必填）；`claim`、`evidence`、`tests_passed`、`files_changed` 和 `max_blocks`（可选）。`blocked` 是已经阻止它停下的次数。 |
| 决策 | `stop`（布尔值）、`reason`，以及用过模型时的 `probability`。 |
| 截止时间 | 默认 3000 毫秒。 |
| 由谁提供 | `agent-stop` 流程，通过 `config/capabilities/agent.stop.yaml`。 |
| 由谁使用 | 对 `POST /v1/hub` 的调用，以及 MCP 服务器的 `agent_stop` 工具。目前还没有 `Stop` 钩子适配器：使用者自己决定如何处理 `stop: false`。 |
| 没有答案 | 模型失败时，流程自己会让智能体停下（`reason: error`）：失效的守卫不应困住它。如果能力中心没有给出答案，你的使用方也应这样做。 |

可能的 `reason` 有 `block-limit`、`no-changes`、`tests-failed`、`verified`、`unverified`、`unsure` 和 `error`。前面几条规则是确定性的，按此顺序求值：连续被阻止 `max_blocks` 次（默认 3）后一律放行，即使测试是红的，以免困住智能体；没有改动文件时无需验证；测试失败时不能停下。只有其余情况才会交给模型，由它判断 `evidence` 是否证明任务完成：低于 0.3 继续（`unverified`），0.3 到 0.6 之间也继续（`unsure`，因为存疑时不停），从 0.6 起可以停下（`verified`）。

## 示例流程

这两个流程位于 `config/flows/`，通过 `POST /v1/decide/{流程}` 运行。它们不是能力中心的能力：没有输入或决策 schema，因此能力中心不会校验它们，也不会通过 MCP 暴露它们。要把其中一个变成能力，需要为它写一份清单，见下文。

| 流程 | 决定什么 | 结果 |
|---|---|---|
| `agent-guard` | 智能体命令的守卫：是否会破坏项目以外的数据？是什么类型的任务？风险有多大？ | `allow`、`deny`、`ask-user` 或 `abstain`。 |
| `model-route` | 根据难度，以及是否涉及安全、生产环境或资金，决定任务该用哪个模型和多大的推理力度。 | `route`，带有 `model` 和 `effort`。 |

[示例](/zh-cn/ejemplos/)中的第 4 个示例讲解了 `model-route`。

## 研究中

以下内容均未实现，也没有日期。列出来是为了分清哪些已是产品，哪些还不是。

| 想法 | 问题 | 为什么合适 |
|---|---|---|
| `model.route` | 这个任务该用哪个模型和力度？ | 已有对应流程；还缺清单和 schema。 |

## 添加一个能力

1. 在 `config/flows/` 中编写流程（用 [`jevalan edit`](/zh-cn/comandos/)，不要手改；见[流程](/zh-cn/flujos/)）。
2. 在清单旁的 `schemas/` 中编写两个 JSON schema：`<能力>.input.json` 和 `<能力>.decision.json`。
3. 在 `hub.capabilities_dir` 中编写清单 `<能力>.yaml`，其中 `provider: { id: jevalan-flow, flow: <流程> }`。
4. 在 `config/lab/fixtures/<能力>/` 中添加 fixture，并通过 `jevalan lab check` 和 `jevalan lab offline`。
5. 在接入之前，先决定使用方在没有答案时的行为（见[指南](/zh-cn/guia-decisiones/)）。

如果加载失败，`GET /v1/hub/capabilities` 会把它列在 `invalid` 下并说明原因；`flow` 类型的能力在其流程缺失期间也会变为无效。
