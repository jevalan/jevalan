---
title: 练习
description: 五个简短的练习，无需网络和密钥，学会用 fixture 固定你的规则所做的决定。
---

这些练习以 `offline` 模式使用[实验室](/zh-cn/laboratorio/)：每个案例都带有手写的模型答案（`replay`），因此测试的是**你的规则和阈值**，而不是任何模型，结果总是一样。不需要守护进程和密钥。请先读完[决策指南](/zh-cn/guia-decisiones/)。

在副本上操作，以免改动 `config/lab`：

```sh
cp -r config/lab mi-lab          # PowerShell：Copy-Item -Recurse config\lab mi-lab
```

## 1. 看看已经固定了什么

```sh
jevalan lab check --lab-dir mi-lab
jevalan lab offline --lab-dir mi-lab
```

`check` 校验文件，`offline` 重放它们。你会看到每个能力一张表，含 `pasa`（通过）、`skipped` 和 `fallan`（失败）。打开 `mi-lab/fixtures/agent.wakeup/judged-sleep.yaml`：一个输入、模型的答案（`worth: { answer: 0.08, … }`）和两条断言，各带 `why`。

**问题：**为什么这个案例让智能体继续休眠，而 `ambiguous-warning`（`answer: 0.65`）却会唤醒它？看看 `config/flows/agent-wakeup.yaml` 里的阈值。

## 2. 写你的第一个 fixture

创建 `mi-lab/fixtures/agent.wakeup/disk-space-sleep.yaml`：

```yaml
id: disk-space-sleep
version: 1
description: "等待构建时的磁盘告警不会唤醒智能体"
input: { waiting_for: "一次完成的构建", event: "告警：磁盘 D: 已用 71 %", skipped: 1 }
replay:
  worth: { answer: 0.12, confidence: 0.9 }
expect:
  - cel: "status == 'ok' && decision.wake == false"
    why: "磁盘告警与构建无关"
```

`id` 与文件名相同。运行 `check` 和 `offline`：你会看到 `lab en orden: 20 fixtures`，`agent.wakeup` 通过的案例从 11 个变成 12 个。

## 3. 故意让它失败

把断言改成 `decision.wake == true`，再运行一次 `offline`：

```text
Falla `disk-space-sleep` (status ok):
- 磁盘告警与构建无关
```

打印出来的就是 `why`。所以应当把它写成**你要保护的规则**，而不是对测试的描述。然后改回 `wake == false`。

## 4. 预测一个边界

把 `replay` 改成 `answer: 0.4999`。运行之前，先在断言里写下你认为会发生什么：休眠？唤醒？`reason` 是什么？`worth` 节点的区间是 `0..0.3`、`0.3..0.5` 和 `0.5..1`，右侧为开区间。

```yaml
expect:
  - cel: "status == 'ok' && decision.wake == true && decision.reason == 'unsure'"
    why: "刚好低于 0.5 时信号存疑，必须唤醒"
```

改完后 fixture 通过：0.4999 仍在存疑区（`unsure`），会唤醒。把它与固定了 0.5 的 `threshold-edge-judged` 对比。如果预测错了，你在用户发现之前就了解了自己的阈值。

## 5. 从真实流量到 fixture

[钩子](/zh-cn/integraciones/)的 `shadow` 模式会记录 Jevalan 本来会做出的决定。把它们变成案例：

```sh
jevalan shadow fixtures --lab-dir mi-lab --max 20
```

它为每个决策写一个 fixture（风险最高的优先），把四个概率记为 `replay`，把决策记为断言：

```yaml
replay:
  risks.destructive:
    answer: 0.62
    confidence: 0.04
  # …
expect:
  - cel: decision.verdict == 'ask'
    why: 记录下来的判定（ask）必须能被复现
```

（以测试日志为例。）从此，只要你改了阈值或规则，`jevalan lab offline` 就会告诉你哪些真实决策会改变。这些 fixture 会保留日志中的 `command`：上传到任何仓库之前，请先检查内容。

## 你学到了什么

- `replay` 固定模型的答案，只测试你的规则。
- `why` 记录规则；阈值的边界用 fixture 固定。
- 真实案例通过 `shadow fixtures` 进入，之后 CI 会在有变化时提醒你。

下一步：[实验室](/zh-cn/laboratorio/)（用 `record` 和 `live` 衡量真实模型）和[能力目录](/zh-cn/catalogo/)（看看还能固定什么）。
