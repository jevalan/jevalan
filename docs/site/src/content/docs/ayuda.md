---
title: Ayuda
description: Guía rápida para probar Jevalan, conectarlo a tus backends y resolver dudas comunes.
---

## Quiero probarlo

Ejecuta la demo; incluye servicios simulados y no necesita claves:

```sh
jevalan tui --demo
```

La TUI es la vista en vivo. No verás actividad hasta que un cliente envíe peticiones.

## Quiero usar mis backends

1. Copia `config.example.yaml` como `config.yaml`.
2. Configura las claves necesarias como variables de entorno.
3. Desde la carpeta del paquete, arranca el daemon: `jevalan serve --config config.yaml`.
4. Abre otra terminal en esa carpeta y ejecuta `jevalan tui`.

Consulta la guía de [configuración](./configuracion/) y [primeros pasos](./primeros-pasos/).

## Quiero conectarlo a una herramienta

Consulta las instrucciones para [Claude Code](https://github.com/jevalan/jevalan/blob/main/docs/claude-code-hook-install.md) o [Crush](https://github.com/jevalan/jevalan/blob/main/docs/crush-hook-install.md). Empieza en modo `shadow` para observar decisiones antes de activar `enforce`.

## Más ayuda

- [Conceptos](./conceptos/): proxy, flujos y capacidades.
- [Comandos](./comandos/): referencia rápida de la CLI.
- [Integraciones](./integraciones/): hooks compatibles.
