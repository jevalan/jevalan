---
title: 配置
description: 配置守护进程、后端以及流程和能力目录。
---

从 `config.example.yaml` 开始。将它复制为 `config.yaml`，并将配置与 `config/flows/`、`config/capabilities/`、`config/signals/` 和 `config/lab/` 放在一起。

```yaml
server:
  addr: "127.0.0.1:8080"
backends:
  ollama:
    url: "http://localhost:11434"
default_backend: ollama
flows_dir: ./config/flows
hub:
  capabilities_dir: ./config/capabilities
  signals_dir: ./config/signals
  allow_exec: false
```

相对路径以当前工作目录为基准。请从解压后的软件包根目录运行 Jevalan，以便找到附带的资源。

## 密钥和访问控制

将密钥保存在环境变量中，并在 YAML 的 `api_key_env` 中填写对应变量名。不要将真实密钥写入已纳入版本控制的文件。如果配置了 `token_env`，请求守护进程时必须携带该令牌。

除非明确希望能力中心启动本地进程，否则请保持 `allow_exec: false`。如果其他用户或进程可以访问守护进程的监听地址，请使用令牌。

完整结构和示例值请参阅 [`config.example.yaml`](https://github.com/jevalan/jevalan/blob/main/config.example.yaml)。
