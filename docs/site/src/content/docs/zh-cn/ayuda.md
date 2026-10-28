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

请参阅[配置](/zh-cn/configuracion/)和[快速入门](/zh-cn/primeros-pasos/)指南。

## 我想连接工具

按照 [Claude Code](/zh-cn/integraciones/) 或 [Crush](/zh-cn/integraciones/) 的说明进行配置。先使用 `shadow` 模式观察决策，再启用 `enforce`。

## 更多帮助

- [基本概念](/zh-cn/conceptos/)：代理、流程和能力。
- [命令](/zh-cn/comandos/)：CLI 快速参考。
- [集成](/zh-cn/integraciones/)：支持的 hooks。
- [架构](/zh-cn/arquitectura/)：各部分如何配合。
- [流程](/zh-cn/flujos/)和[能力与能力中心](/zh-cn/capacidades/)：定义你的决策。
- [HTTP API](/zh-cn/api/)：所有路由，附示例。
- [实验室](/zh-cn/laboratorio/)：测试并衡量它们。
- [故障排查](/zh-cn/solucion-de-problemas/)：出问题时查看。
