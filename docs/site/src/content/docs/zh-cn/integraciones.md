---
title: 集成
description: 通过 hooks 将 Jevalan 连接到 Claude Code 和 Crush。
---

Jevalan 提供 hook 适配器，可将客户端输入发送到本地能力中心，并将结果转换为客户端所需的格式。

- [安装 Claude Code hook](https://github.com/jevalan/jevalan/blob/main/docs/claude-code-hook-install.md)
- [安装 Crush hook](https://github.com/jevalan/jevalan/blob/main/docs/crush-hook-install.md)

Hooks 可运行于 `shadow` 模式，只记录决策而不阻止操作；也可运行于 `enforce` 模式，遵循决策结果。启用 `enforce` 前请检查配置。
