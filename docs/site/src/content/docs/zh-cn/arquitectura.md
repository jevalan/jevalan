---
title: 架构
description: 代理、流程引擎、能力中心、事件总线和实验室是如何配合的。
---

Jevalan 是一个本地守护进程：有三个入口，中间是一个引擎，外加一个由终端界面监视的事件流。

```text
        agents, hooks, scripts
                │
   ┌────────────┼──────────────────────────┐
   │            │                          │
POST /{backend}/v1/systemone   POST /v1/decide/{flow}   POST /v1/hub
(raw System One question)      (one specific flow)      (by capability)
   │            │                          │
┌──▼────────────▼──────────────────────────▼─────────────────────┐
│                           JEVALAN                              │
│                                                                │
│  passthrough ─────────┐   flow engine ◄──── capability registry│
│  (forwards untouched) │   rule · noul ·      ├─ flow  → your flows
│                       │   choice · score ·   ├─ http  → a service
│                       │   batch              └─ exec  → a process
│                       ▼          │                             │
│           System One backends (OpenRouter, TypeSafe, Ollama,   │
│           local models): keys, deadlines, cost                 │
│                                                                │
│  event bus ──── WebSocket /events ────► terminal UI            │
└────────────────────────────────────────────────────────────────┘
                          ▲
                          │ same path, in process, no HTTP
                  jevalan lab (offline · record · live)
```

## 三条原则

1. **模型判断，你的代码行动。** 模型返回类型化的概率。你能阅读和测试的本地规则与阈值，把它们转化为决策。由智能体去执行效果。Jevalan 从不执行。
2. **智能体不知道是谁在做决策。** 它请求一个能力。由你自己的流程还是第三方专家来回答，是配置中的选择，所以你可以更换而无需改动智能体。
3. **遥测是显式的。** 每一步都会发出事件，你可以在 TUI 中看到。事件中没有保存原始输入的字段，但可能包含答案、结果、信号和错误，所以请把这个事件流视为敏感数据。

## 各个组成部分

| 组成部分 | 作用 |
|---|---|
| **Passthrough** | 把 System One 请求原样转发给后端，并负责保管密钥、截止时间和成本核算。 |
| **流程引擎** | 遍历由规则和模型提问组成的 YAML 图，并返回结果以及它所走的路径。 |
| **能力注册表** | 把一个类型化的问题映射到提供者：流程、HTTP 服务或本地进程。 |
| **事件总线** | 通过 WebSocket 逐节点发布发生的一切。 |
| **终端界面** | 实时显示拓扑、指标、决策和追踪，并把流程作为草稿来编辑。 |
| **实验室** | 让相同的用例走能力中心的路径，可以离线，也可以针对真实后端。 |
| **Hook 适配器** | 让 Claude Code 和 Crush 在每次工具调用之前查询 `tool.risk`。 |

## 一个决策的完整过程

假设一个 Claude Code 会话即将运行 `rm -rf /tmp/build`：

1. Claude Code 运行 `PreToolUse` hook，hook 从 stdin 读取这次工具调用。
2. hook 通过发现文件找到守护进程，并为 `tool.risk` 发送 `POST /v1/hub`。
3. 能力中心把 `tool.risk` 解析到 `tool-risk-local` 流程。
4. 先运行两条固定规则：递归删除根目录或主目录会被拒绝，下载内容通过管道交给解释器则会询问。这两条都不调用模型。
5. 如果都不匹配，一个 `batch` 节点会在一次本地模型调用中提出四个风险问题，得到四个概率。
6. 路由规则应用阈值：任何风险达到 0.85 或以上为 `deny`，达到 0.55 或以上为 `ask`，否则为 `allow`。
7. 能力中心根据 schema 校验决策并返回。每一步都已作为事件发布。
8. hook 把决策记录到影子日志中。在 `enforce` 模式下它还会回应 Claude Code；在 `shadow` 模式下保持沉默。

hook 只能收紧 Claude Code 的权限，永远不能放宽。最坏的情况只是多问了几次。

## 深入了解

- [流程](/zh-cn/flujos/)和[能力与能力中心](/zh-cn/capacidades/)介绍了定义决策的两种方式。
- [实验室](/zh-cn/laboratorio/)介绍如何衡量一个决策。
- 仓库中有包含更详细示意图的完整设计：[`docs/architecture.md`](https://github.com/jevalan/jevalan/blob/main/docs/architecture.md)。
