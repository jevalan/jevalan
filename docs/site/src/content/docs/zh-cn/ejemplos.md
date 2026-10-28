---
title: 示例
description: 五个完整示例：从只用规则的流程，到守护代码智能体，再到 CI 中的检查。
---

每个示例都是自包含的。前两个是针对真实守护进程运行的，所展示的响应就是它的实际输出；其余示例使用仓库自带的文件和命令。

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

## 5. 在 CI 中检查决策

把实验室加入你的流水线。它不需要后端，也不需要密钥：

```sh
jevalan lab check && jevalan lab offline
```

`lab check` 校验 fixture（在本仓库中会输出 `lab en orden: 16 fixtures`）。`lab offline` 重放它们，并为每个能力输出一张表，包含 `pasa`、`skipped` 和 `fallan` 列；只要有用例失败，就会以非零状态退出。

要在流程到达守护进程之前校验它，请把它发送到 `POST /v1/flows/validate`。无效的流程会返回 `ok: false` 以及确切的问题，包括 CEL 表达式中的位置。参见 [API](/zh-cn/api/) 和[实验室](/zh-cn/laboratorio/)。
