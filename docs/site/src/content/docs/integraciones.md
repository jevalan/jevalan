---
title: Integraciones
description: Conecta Jevalan con Claude Code y Crush mediante sus hooks.
---

Jevalan incluye adaptadores de hook que envían la entrada del cliente al hub local y traducen el resultado al formato que espera ese cliente.

- [Instalar el hook de Claude Code](https://github.com/jevalan/jevalan/blob/main/docs/claude-code-hook-install.md)
- [Instalar el hook de Crush](https://github.com/jevalan/jevalan/blob/main/docs/crush-hook-install.md)

Los hooks pueden configurarse en modo `shadow` para registrar decisiones sin bloquear, o en modo `enforce` para respetar el veredicto. Revisa la configuración antes de activar `enforce`.
