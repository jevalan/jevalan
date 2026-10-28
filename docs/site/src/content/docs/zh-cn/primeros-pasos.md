---
title: 快速入门
description: 安装 Jevalan、运行演示并启动本地守护进程。
---

Jevalan 是一个终端程序和本地守护进程。本网站仅用于项目介绍和使用帮助；请求都在你的设备上处理。

## 下载

发布版本后，可从 [GitHub Releases](https://github.com/jevalan/jevalan/releases) 下载适用于你系统的软件包。解压完整压缩包，并在该目录中运行命令：其中包含可执行文件、流程、能力和示例配置。

如果尚无发布版本，请克隆仓库并安装 Rust 1.89 或更高版本进行编译：

```sh
git clone https://github.com/jevalan/jevalan.git
cd jevalan
cargo build --locked --release -p jevalan
```

## 查看演示

演示集成了守护进程、模拟后端和模拟流量，无需配置凭据：

```sh
# Windows PowerShell
.\target\release\jevalan.exe tui --demo

# Linux 和 macOS
./target/release/jevalan tui --demo
```

## 使用自己的后端启动

在解压后的目录中复制示例配置，并检查其中的端点：

```powershell
Copy-Item config.example.yaml config.yaml
```

将所需后端密钥设为环境变量，然后启动守护进程：

```sh
jevalan serve --config config.yaml
```

在同一目录中打开另一个终端并运行 `jevalan tui`，即可查看状态和事件。守护进程不会自行生成请求；请求由调用它的客户端发送。

示例文件指向外部服务商和本地 Ollama。本指南中只有演示模式保证不会调用真实服务。
