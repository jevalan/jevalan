---
title: 故障排查
description: 解决守护进程、流程、hook 和实验室中最常见的问题。
---

## 流程或能力显示为“无效”

守护进程启动时会为每个流程和能力打印一行，`GET /v1/flows` 和 `GET /v1/hub/capabilities` 会在 `invalid` 下列出无效项及其原因：

```text
flujo cargado: ticket-triage (2 nodos)
flujo inválido: agent-wakeup: worth: backend desconocido "openrouter"
capacidad inválida: agent.wakeup: proveedor inválido: flujo desconocido: agent-wakeup
```

常见原因是流程指定的后端没有在你的配置中定义（`backend desconocido`）。请把该后端添加到配置的 `backends:` 中，如果需要密钥也一并设置。由流程提供服务的能力，在该流程无效期间也是无效的，所以修好流程就同时修好了两者。

使用只定义了部分后端的最小配置时，这是预期的情况。它不是故障，但需要缺失后端的流程将不会被提供服务。

## 守护进程无法启动

- **缺少变量。** 如果 `default_backend` 未定义、某个 `url` 不是 http 或 https，或者某个 `api_key_env` 所指的变量为空或未设置，守护进程会立即失败。错误信息以 `configuración inválida:` 开头，会说明是哪一个，并且守护进程以退出码 `2` 结束。
- **端口被占用。** 守护进程以退出码 `1` 结束，并给出操作系统的“地址已被使用”错误（Windows 上为 `os error 10048`）。修改 `server.addr`，或设置 `JEVALAN_ADDR`。
- **相对路径。** `flows_dir`、`capabilities_dir` 和 `signals_dir` 是从你启动守护进程的文件夹开始解析的。请从项目根目录启动，或者使用绝对路径并同时设置 `signals_dir`，否则 `tool-risk-local` 无法加载。不存在的目录不算错误：守护进程会启动，打印类似 `aviso: no existe el directorio de flujos ./flows` 的警告，并且不会从中提供任何内容。

## 终端界面什么都不显示

TUI 只是一个查看器：只有当客户端发送请求时才会出现活动。请按顺序检查：

1. 守护进程在运行吗？`curl http://127.0.0.1:8080/healthz` 应该输出 `ok`。
2. 守护进程要求令牌吗？传入 `--token` 或设置 `JEVALAN_TOKEN`。没有传入时，TUI 会从发现文件中获取令牌。
3. 想在没有其他任何东西的情况下看它运行？运行 `jevalan tui --demo`，它自带服务和流量。

## 请求返回 `502`

后端不可达，或者某个节点失败且没有 `error` 端口。检查后端的 URL 和密钥，并为该节点添加 `error` 端口，让流程有意地处理失败，而不是直接结束请求。

## hook 什么都没做

这正是 `shadow` 模式的用途：它不输出任何内容，并且始终以 0 退出。要检查它是否工作：

- 查看影子日志（数据目录中的 `shadow.jsonl`），或在 TUI 中按 `H`：调用应该出现在那里。
- 如果什么都没有记录，很可能是守护进程没有运行。hook 通过读取 `daemon.json` 来找到它，没有这个文件就不会尝试连接。
- 守护进程被强行终止（而不是正常停止）时遗留下来的 `daemon.json`，会指向一个已经不再响应的端口。请重新启动守护进程，或删除该文件。
- 在 `enforce` 模式下，能力中心无法决定的一切情况（无守护进程、弃权、错误、超时）都会返回 `ask`，所以守护进程停止期间你会被更频繁地询问。

## `exec` 能力返回 `provider_disabled`

运行进程默认是关闭的。如果你信任该能力，请设置 `hub.allow_exec: true`（或 `JEVALAN_ALLOW_EXEC=1`）。如果命令中使用了 `${TOOLGATE_CLI}` 这样的变量而它没有定义，那么在 `allow_exec` 开启时该能力会变为无效。

## `lab offline` 失败

- **“no se ejecutó ningún fixture”（没有运行任何 fixture）** 表示所有 fixture 都被过滤或跳过了。检查 `modes`，以及 cassette 是否已过期。
- **某个 fixture 因 cassette 过期而 `skipped`。** 你修改了该 fixture 的 `version` 或 `input`。请为它重新运行 `jevalan lab record`。
- **`lab check` 拒绝某个 `replay` 键。** 该键必须是流程中的某个节点，或提供者的 `id`。

## 从源码构建

- **Windows 与 Git Bash：** Git 自带的 `link.exe` 会遮盖 MSVC 链接器，构建会因链接器错误而失败。仅为该命令用干净的 `PATH` 运行 Cargo：`PATH="$(dirname "$(command -v cargo)"):/c/Windows/System32:/c/Windows" cargo build`。
- **磁盘空间不足**（Windows 错误 112）：用 `CARGO_TARGET_DIR` 把 Cargo 的输出放到有空间的卷上。
- **Rust 版本：** 需要 Rust 1.89 或更高版本。

仍然卡住？请[提交 issue](https://github.com/jevalan/jevalan/issues)，附上你运行的命令和守护进程的输出，并事先删除所有密钥。如果是安全问题，请改为遵循[安全政策](https://github.com/jevalan/jevalan/blob/main/SECURITY.md)。
