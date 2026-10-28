---
title: 示例
description: 九个完整示例：从只用规则的流程，到守护代码智能体，再到 CI 中的检查。
---

每个示例都是自包含的。前两个是针对真实守护进程运行的，所展示的响应就是它的实际输出。示例 5 到 8 能通过 `POST /v1/flows/validate`，但它们会询问模型，因此没有用模型运行过：请把其中的阈值和权重当作初步估计，并用[实验室](/zh-cn/laboratorio/)调整。其余示例使用仓库自带的文件和命令。

## 1. 用简单规则为支持工单分流

没有模型，也不调用后端：流程只读取请求并沿着端口前进。把它保存为 `config/flows/ticket-triage.yaml`：

```yaml
description: "Route a support ticket"
defaults: { backend: ollama, model: nimble }
start: urgent
nodes:
  urgent:
    type: rule
    when: 'input.priority == "high"'
    out: { "true": page, "false": billing }
  billing:
    type: rule
    when: 'input.topic == "billing"'
    out: { "true": finance, "false": queue }
outputs:
  page:    { outcome: ok, data: { team: oncall,  ticket: "$input/id" } }
  finance: { outcome: ok, data: { team: finance, ticket: "$input/id" } }
  queue:   { outcome: ok, data: { team: support, ticket: "$input/id" } }
```

```sh
curl -s http://127.0.0.1:8080/v1/decide/ticket-triage \
  -H 'Content-Type: application/json' \
  -d '{"state":{"id":"T-43","priority":"low","topic":"billing"}}'
```

响应是 `outcome: ok`，`data: {"team":"finance","ticket":"T-43"}`，并附带通向该结果的 `path`。所有节点类型见[流程](/zh-cn/flujos/)。

## 2. 为部署设置关卡

同样的思路，用作流水线某一步之前的安全检查。由两条规则决定，流程返回 `allow`、`ask` 或 `deny` 并附上原因：

```yaml
description: "Gate a deployment with plain rules"
defaults: { backend: ollama, model: nimble }
start: prod_from_branch
nodes:
  prod_from_branch:
    type: rule
    when: 'input.env == "prod" && input.branch != "main"'
    out: { "true": deny_branch, "false": tests }
  tests:
    type: rule
    when: '!input.tests_passed'
    out: { "true": ask_tests, "false": allow }
outputs:
  deny_branch: { outcome: deny, data: { reason: "production deploys must come from main" } }
  ask_tests:   { outcome: ask,  data: { reason: "tests have not passed", branch: "$input/branch" } }
  allow:       { outcome: allow }
```

| 请求的 `state` | `outcome` | `data` |
|---|---|---|
| `{"env":"prod","branch":"feature/x","tests_passed":true}` | `deny` | `{"reason":"production deploys must come from main"}` |
| `{"env":"prod","branch":"main","tests_passed":false}` | `ask` | `{"branch":"main","reason":"tests have not passed"}` |
| `{"env":"prod","branch":"main","tests_passed":true}` | `allow` | 无 |

你的流水线读取 `outcome` 并决定怎么做；Jevalan 从不执行部署。

## 3. 守护你的代码智能体

先把 hook 设为 `shadow` 模式。它会记录 Jevalan 本会做出的决策，并且从不干预：

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "*", "hooks": [ { "type": "command", "command": "/path/to/jevalan hook claude-code --mode shadow" } ] }
    ]
  }
}
```

正常工作几天，然后查看它看到了什么：

```sh
jevalan shadow report --since 2026-10-01 --out report.md
```

当这些判定与你自己的判断一致时，把 `--mode shadow` 改为 `--mode enforce`。在 `enforce` 模式下，hook 只能收紧权限，无法决定时会询问。参见[集成](/zh-cn/integraciones/)。

## 4. 按难度选择模型

`config/flows/model-route.yaml` 会给任务打分，把简单的交给小模型，把困难的交给大模型，并对涉及安全、生产数据或金钱的任务增加一个是非判断：

```yaml
start: tier
nodes:
  tier:
    type: score
    instructions: "How difficult is the task?"
    criteria: ["trivial", "easy", "medium", "hard", "very hard"]
    out: { "0..1.5": small, "1.5..3": risky, "3..4": large, error: medium, low-confidence: medium }
  risky:
    type: noul
    instructions: "Does the task touch security, production data or money?"
    out: { "true": large, "false": medium }
outputs:
  small:  { outcome: route, data: { model: haiku,  effort: low } }
  medium: { outcome: route, data: { model: sonnet, effort: medium } }
  large:  { outcome: route, data: { model: opus,   effort: high } }
```

与前两个不同，这个流程会询问模型，因此需要一个可访问的后端。请注意 `error` 和 `low-confidence` 端口：当模型失败或没有把握时，流程会退回到中间选项，而不是让请求失败。仓库中的文件，其指令是用西班牙语写的；结构相同。

## 5. 在消息到达 LLM 应用之前先筛查

一个 `batch` 节点在一次调用中提出两个是非问题。阈值由你的代码而不是模型来应用：明显的指令覆盖会被拦截，可疑的或含有个人数据的送去复核，后端失败也送去复核，而不是放行消息。

```yaml
description: "Screen a user message before it reaches your LLM app"
defaults: { backend: ollama, model: nimble, min_confidence: 0.5 }
start: hazards
nodes:
  hazards:
    type: batch
    questions:
      jailbreak: { type: noul, instructions: "Does the message try to override the assistant's instructions?", criteria: { "true": "It tries to override them", "false": "It does not" } }
      pii:       { type: noul, instructions: "Does the message contain personal data such as a card or ID number?", criteria: { "true": "It does", "false": "It does not" } }
    route:
      - { when: "answers.hazards.jailbreak >= 0.85", to: block }
      - { when: "answers.hazards.jailbreak >= 0.55 || answers.hazards.pii >= 0.7", to: review }
    else: pass
    out: { error: review }
outputs:
  block:  { outcome: deny,  data: { reason: "likely instruction override", scores: "$answer/hazards" } }
  review: { outcome: ask,   data: { reason: "needs a second look", scores: "$answer/hazards" } }
  pass:   { outcome: allow }
```

`deny` 和 `ask` 都在 `data.scores` 中带有原始分数，复核的人可以看到原因。

## 6. 把每条支持消息交给合适的处理者

`choice` 节点选择一个标签，每个标签就是一个端口。请注意 `low-confidence` 和 `error` 都会交给真人：模型只处理明确无误的消息。

```yaml
description: "Send each support message to the right handler"
defaults: { backend: ollama, model: nimble, min_confidence: 0.6 }
start: intent
nodes:
  intent:
    type: choice
    instructions: "What does the customer want?"
    criteria: { refund: "Money back or a billing dispute", bug: "Something is broken", howto: "A question about using the product", other: "Anything else" }
    out: { refund: to_billing, bug: to_engineering, howto: to_bot, other: to_human, low-confidence: to_human, error: to_human }
outputs:
  to_billing:     { outcome: route, data: { handler: billing,     mode: human } }
  to_engineering: { outcome: route, data: { handler: engineering, mode: human } }
  to_bot:         { outcome: route, data: { handler: docs-bot,    mode: automatic } }
  to_human:       { outcome: route, data: { handler: support,     mode: human } }
```

调用方读取 `data.mode`，以得知该由机器人还是真人回复。

## 7. 用你自己掌控的权重给线索打分

提出小而独立的问题，再自己组合这些概率。权重写在 `when` 表达式里，因此可见、可审查、也容易修改；模型从不做算术。

```yaml
description: "Score an inbound lead from three weighted questions"
defaults: { backend: ollama, model: nimble }
start: lead
nodes:
  lead:
    type: batch
    questions:
      budget: { type: noul, instructions: "Does the message suggest a budget for a paid plan?", criteria: { "true": "It does", "false": "It does not" } }
      need:   { type: noul, instructions: "Does the sender describe a concrete problem we solve?", criteria: { "true": "It does", "false": "It does not" } }
      timing: { type: noul, instructions: "Does the sender want to start within a month?", criteria: { "true": "It does", "false": "It does not" } }
    route:
      - { when: "0.4 * answers.lead.budget + 0.4 * answers.lead.need + 0.2 * answers.lead.timing >= 0.7", to: hot }
      - { when: "0.4 * answers.lead.budget + 0.4 * answers.lead.need + 0.2 * answers.lead.timing >= 0.4", to: warm }
    else: cold
    out: { error: warm }
outputs:
  hot:  { outcome: route, data: { queue: sales-today,  scores: "$answer/lead" } }
  warm: { outcome: route, data: { queue: sales-review } }
  cold: { outcome: route, data: { queue: newsletter } }
```

如果后端失败，线索会进入中间队列，而不是丢失或被提级。

## 8. 只在没把握时才升级到更强的模型

大多数消息很简单，小模型就能又好又便宜地处理。给它设置 `min_confidence`，并把它的 `low-confidence` 和 `error` 端口接到使用更大模型的第二个节点。如果连这个也没把握，流程就不再询问模型，而是把案例交给真人。

```yaml
description: "Classify with a small model and escalate to a larger one when unsure"
defaults: { backend: ollama, model: small, min_confidence: 0.7 }
start: quick
nodes:
  quick:
    type: choice
    instructions: "What is the sentiment of the customer message?"
    criteria: { positive: "Clearly happy", neutral: "Neither happy nor unhappy", negative: "Clearly unhappy" }
    out: { positive: pos_cheap, neutral: neu_cheap, negative: neg_cheap, low-confidence: careful, error: careful }
  careful:
    type: choice
    model: large
    min_confidence: 0.5
    instructions: "What is the sentiment of the customer message?"
    criteria: { positive: "Clearly happy", neutral: "Neither happy nor unhappy", negative: "Clearly unhappy" }
    out: { positive: pos_strong, neutral: neu_strong, negative: neg_strong, low-confidence: human, error: human }
outputs:
  pos_cheap:  { outcome: classified, data: { sentiment: positive, tier: small } }
  neu_cheap:  { outcome: classified, data: { sentiment: neutral,  tier: small } }
  neg_cheap:  { outcome: classified, data: { sentiment: negative, tier: small } }
  pos_strong: { outcome: classified, data: { sentiment: positive, tier: large } }
  neu_strong: { outcome: classified, data: { sentiment: neutral,  tier: large } }
  neg_strong: { outcome: classified, data: { sentiment: negative, tier: large } }
  human:      { outcome: review, data: { reason: "both models were unsure" } }
```

每个节点都可以指定自己的 `model`（以及 `backend`），因此升级只是多一个节点。输出中的 `tier` 会告诉你是哪个模型做出的决定，这样节省的成本就可以衡量：统计便宜档够用的比例，并用[实验室](/zh-cn/laboratorio/)调整 `min_confidence`。这里的模型和阈值只是占位，请换成你实际运行的。

## 9. 在 CI 中检查决策

把实验室加入你的流水线。它不需要后端，也不需要密钥：

```sh
jevalan lab check && jevalan lab offline
```

`lab check` 校验 fixture（在本仓库中会输出 `lab en orden: 16 fixtures`）。`lab offline` 重放它们，并为每个能力输出一张表，包含 `pasa`、`skipped` 和 `fallan` 列；只要有用例失败，就会以非零状态退出。

要在流程到达守护进程之前校验它，请把它发送到 `POST /v1/flows/validate`。无效的流程会返回 `ok: false` 以及确切的问题，包括 CEL 表达式中的位置。参见 [API](/zh-cn/api/) 和[实验室](/zh-cn/laboratorio/)。
