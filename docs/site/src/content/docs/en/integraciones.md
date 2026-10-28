---
title: Integrations
description: Connect Jevalan to Claude Code and Crush using hooks.
---

Jevalan includes hook adapters that send client input to the local hub and translate the result into the format expected by that client.

- [Install the Claude Code hook](https://github.com/jevalan/jevalan/blob/main/docs/claude-code-hook-install.md)
- [Install the Crush hook](https://github.com/jevalan/jevalan/blob/main/docs/crush-hook-install.md)

Hooks can run in `shadow` mode to record decisions without blocking, or in `enforce` mode to follow the verdict. Review the configuration before enabling `enforce`.
