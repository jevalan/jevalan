---
title: 命令
description: TUI、守护进程、流程编辑器、hook、影子日志和实验室的命令。
---

| 命令 | 用途 |
|---|---|
| `jevalan tui --demo` | 打开带有守护进程、模拟服务和模拟流量的 TUI。无需密钥。 |
| `jevalan tui --fake` | 打开使用确定性序列的 TUI，不需要守护进程或后端。 |
| `jevalan tui` | 连接到有响应的守护进程，或针对你的真实后端启动一个。 |
| `jevalan serve --config config.yaml` | 启动本地守护进程。 |
| `jevalan edit config/flows/<flow>.yaml` | 以草稿方式编辑流程。 |
| `jevalan hook claude-code [--mode shadow\|enforce]` | 用于 Claude Code 的 `PreToolUse` hook。 |
| `jevalan hook crush [--mode shadow\|enforce]` | 用于 Crush 的 hook。 |
| `jevalan hook run --client <name> --dialect <claude\|crush>` | 通用 hook 适配器。 |
| `jevalan shadow report` | 针对影子日志的 Markdown 报告。 |
| `jevalan shadow fixtures` | 把真实决策转成实验室 fixture。 |
| `jevalan mcp [--allow-live]` | 基于 stdio 的 MCP 服务器：把能力中心的能力暴露为工具。见 [MCP 服务器](/zh-cn/servidor-mcp/)。 |
| `jevalan lab check` | 离线校验实验室 fixture。 |
| `jevalan lab offline` | 离线重放 fixture。 |
| `jevalan lab record` | 调用真实后端并录制 cassette，受预算限制。 |
| `jevalan lab live` | 带重复次数地衡量真实后端，受预算限制。 |
| `jevalan --version` | 显示已安装的版本。 |

运行 `jevalan <command> --help` 可查看某个命令的全部选项。

## 终端界面选项

| 参数 | 含义 | 默认值 |
|---|---|---|
| `--url` | 守护进程的事件 WebSocket。 | `ws://127.0.0.1:8080/events` |
| `--token` | 守护进程的令牌（也可用 `JEVALAN_TOKEN`）。 | 无 |
| `--fx` | 视觉效果：`off`、`events` 或 `full`（也可用 `JEVALAN_FX`）。 | `events` |
| `--fake` | 用模拟追踪为 TUI 提供数据。 | 关闭 |
| `--fake-rate`、`--fake-seed` | 每秒模拟请求数，以及用于可重复运行的种子。 | `2`、随机 |
| `--demo` | 模拟后端和模拟流量。 | 关闭 |
| `--config` | TUI 启动守护进程时使用的配置文件（如果需要启动的话）。 | 无 |

### 按键

| 按键 | 操作 |
|---|---|
| `q` | 退出。 |
| `?` | 帮助。 |
| `p` | 暂停或继续。 |
| `f` | 循环切换效果：off、events、full。 |
| `esc` | 返回。 |
| `tab` | 在拓扑和事件流之间切换焦点。 |
| `enter` | 打开流程（在拓扑端口上）或追踪（在事件流的某一行上）。 |
| 方向键或 `hjkl` | 移动视图。 |
| `+` `-` `0` | 放大、缩小、适应视图。 |
| `n` `N` | 下一个或上一个节点。 |
| `t` | 在追踪中，切换节点路径和事件时间线。 |
| `H` | hook 决策的实时视图。 |
| `x` | 在 `--demo` 中，让选中的后端宕机或恢复。 |

## 流程编辑器按键

`jevalan edit` 是针对单个流程文件的终端编辑器。它处理的是草稿：从不发布，从不触碰守护进程的注册表，未通过校验的草稿也绝不会写入磁盘。

| 按键 | 操作 |
|---|---|
| `tab` | 在图、检查器和底部状态栏之间移动。 |
| 方向键或 `hjkl` | 移动选择。 |
| `e` | 编辑选中的字段。`enter` 在草稿中确认修改。 |
| `a` | 添加节点：`rule`、`noul`、`choice`、`score` 或 `batch`。 |
| `d` | 删除选中的节点，需要确认。 |
| `c` | 重新连接选中的端口。 |
| `s` | 以补丁方式保存，并保留 `.bak` 副本。注释和顺序会保留。 |
| `g` | 当文件在底层发生变化时，另存为 `<name>.editado.yaml`。 |
| `r` | 从磁盘重新加载，丢弃草稿。 |
| `q` | 退出，有未保存的修改时会给出警告。 |
| `esc` | 取消当前的编辑、菜单或确认。 |

`config/flows/tool-risk-local.yaml` 参与 hook 的决策。编辑它之前，请先在副本上练习。
