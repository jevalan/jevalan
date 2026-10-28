---
title: 命令
description: TUI、守护进程、流程和实验室的常用命令。
---

| 命令 | 用途 |
|---|---|
| `jevalan tui --demo` | 启动包含守护进程、模拟服务和模拟流量的 TUI。 |
| `jevalan tui --fake` | 使用确定性序列启动 TUI，无需守护进程或后端。 |
| `jevalan serve --config config.yaml` | 启动本地守护进程。 |
| `jevalan edit config/flows/<flow>.yaml` | 以草稿方式编辑流程。 |
| `jevalan lab check` | 离线验证实验室 fixtures。 |
| `jevalan lab offline` | 离线重放 fixtures。 |
| `jevalan --version` | 显示已安装的版本。 |

`config/flows/tool-risk-local.yaml` 会参与 hook 决策。如需编辑，请先用副本练习。

运行 `jevalan <command> --help` 查看命令的所有选项。
