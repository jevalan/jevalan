---
title: 使用帮助
description: 快速了解如何试用 Jevalan、连接后端并查找常见问题的答案。
---

## 我想先试用

运行演示即可。演示包含模拟服务，无需密钥：

```sh
jevalan tui --demo
```

TUI 是实时视图。只有客户端发送请求后才会显示活动。

## 我想使用自己的后端

1. 将 `config.example.yaml` 复制为 `config.yaml`。
2. 将所需密钥设为环境变量。
3. 在软件包目录中启动守护进程：`jevalan serve --config config.yaml`。
4. 在该目录的另一个终端中运行 `jevalan tui`。

请参阅[配置](./configuracion/)和[快速入门](./primeros-pasos/)指南。

## 我想连接工具

按照 [Claude Code](https://github.com/jevalan/jevalan/blob/main/docs/claude-code-hook-install.md) 或 [Crush](https://github.com/jevalan/jevalan/blob/main/docs/crush-hook-install.md) 的说明进行配置。先使用 `shadow` 模式观察决策，再启用 `enforce`。

## 更多帮助

- [基本概念](./conceptos/)：代理、流程和能力。
- [命令](./comandos/)：CLI 快速参考。
- [集成](./integraciones/)：支持的 hooks。
