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

See [Configuration](/en/configuracion/) and [Getting started](/en/primeros-pasos/).

## I want to connect a tool

Follow the instructions for [Claude Code](/en/integraciones/) or [Crush](/en/integraciones/). Start in `shadow` mode to observe decisions before enabling `enforce`.

## More help

- [Concepts](/en/conceptos/): proxy, flows, and capabilities.
- [Commands](/en/comandos/): a quick CLI reference.
- [Integrations](/en/integraciones/): supported hooks.
- [Architecture](/en/arquitectura/): how the pieces fit.
- [Flows](/en/flujos/) and [Capabilities and hub](/en/capacidades/): define your decisions.
- [HTTP API](/en/api/): every route, with examples.
- [Lab](/en/laboratorio/): test and measure them.
- [Troubleshooting](/en/solucion-de-problemas/): when something does not work.
