---
title: Help
description: A quick guide to trying Jevalan, connecting your backends, and finding answers.
---

## I want to try it

Run the demo. It includes mock services and needs no keys:

```sh
jevalan tui --demo
```

The TUI is the live view. Activity appears when a client sends requests.

## I want to use my backends

1. Copy `config.example.yaml` to `config.yaml`.
2. Set the required keys as environment variables.
3. From the package directory, start the daemon: `jevalan serve --config config.yaml`.
4. Open another terminal in that directory and run `jevalan tui`.

See [Configuration](./configuracion/) and [Getting started](./primeros-pasos/).

## I want to connect a tool

Follow the instructions for [Claude Code](https://github.com/jevalan/jevalan/blob/main/docs/claude-code-hook-install.md) or [Crush](https://github.com/jevalan/jevalan/blob/main/docs/crush-hook-install.md). Start in `shadow` mode to observe decisions before enabling `enforce`.

## More help

- [Concepts](./conceptos/): proxy, flows, and capabilities.
- [Commands](./comandos/): a quick CLI reference.
- [Integrations](./integraciones/): supported hooks.
