---
title: MCP 服务器
description: 用一条命令，让任何 MCP 客户端向 Jevalan 请求决策，并列出、校验和试运行流程。
---

`jevalan mcp` 是 [Model Context Protocol](https://modelcontextprotocol.io) 与守护进程 HTTP API 之间的一座小桥。Claude Code 这样的 MCP 客户端通过 stdio 启动它，能力中心已加载的每个能力都会出现为一个工具。它本身不含任何决策逻辑：每次调用都会转发给守护进程，守护进程继续应用自己的期限、校验和事件。

## 连接客户端

守护进程必须正在运行（`jevalan serve`）。这座桥找到它的方式与 hook 相同：通过发现文件（`daemon.json`），没有该文件时使用默认地址。

对于 Claude Code，把它加入项目的 `.mcp.json`，并使用二进制副本的**绝对路径**：

```json
{
  "mcpServers": {
    "jevalan": { "command": "/path/to/jevalan", "args": ["mcp"] }
  }
}
```

| 选项 | 含义 |
|---|---|
| `--addr host:port` | 只与这个守护进程通信，忽略 `daemon.json`。 |
| `--token` | 守护进程的令牌（也可从 `JEVALAN_TOKEN` 读取）。从不打印。 |
| `--allow-live` | 允许 `jevalan_try_draft` 使用 `live` 和 `record` 传输。见下文。 |

## 工具

**每个能力一个工具。** 每个已加载的能力都会成为一个工具，其输入 schema 就是该能力自己的 schema。名称是能力 id，其中所有不是字母、数字、`_` 或 `-` 的字符都被替换为 `_`，因此 `tool.risk` 变成 `tool_risk`，`agent.wakeup` 变成 `agent_wakeup`。结果是能力中心的类型化答复：`status` 为 `ok`、`abstain` 或 `error`，只有 `error` 会被标记为调用失败。当守护进程重新加载能力时，客户端会收到列表已变化的通知。

**四个用于流程的工具**，始终存在：

| 工具 | 作用 |
|---|---|
| `jevalan_run_flow` | 按名称用一个 `state` 运行已发布的流程，并返回它的 `outcome`、`data` 和 `path`。 |
| `jevalan_list_flows` | 列出已加载的流程和无效的流程，并附上原因。 |
| `jevalan_validate_flow` | 用真实的解析器校验流程的 YAML，但不发布。无效的流程会返回 `ok: false` 和问题所在；这是一个判定，而不是失败的调用。 |
| `jevalan_try_draft` | 把草稿运行一次，但不发布。 |

## 它特意不做的事

- 它不能保存、编辑或删除流程，也不能改动 `config/signals/`。修改流程仍然是人的工作，使用 [`jevalan edit`](/zh-cn/comandos/) 完成。安装了 Claude Code hook 时，`tool-risk-local` 是真的在做决定，而让智能体改写自己的安全规则会让这一切失去意义。
- 它从不启动进程，也不会自己打开任何网络端口。
- 如果守护进程不存在或不再响应，调用会带着原因以错误返回。它从不编造判定。

## 关于 `--allow-live`

`jevalan_try_draft` 接受一个 `transport`。默认只提供 `replay`：重放已录制的答案，不调用任何东西。`live` 和 `record` 会调用你真实的后端，可能产生费用，因此除非你用 `--allow-live` 启动这座桥，否则会被拒绝。

## 询问不等于强制

智能体现在可以在执行之前用 `tool_risk` 查询某次调用。这是一种便利，而不是一道屏障：被客气询问的模型，同样可以决定不去问。若要强制执行决策，请继续使用 [hook](/zh-cn/integraciones/)。
