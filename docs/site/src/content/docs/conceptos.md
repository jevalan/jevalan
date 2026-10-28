---
title: Conceptos
description: Qué hace Jevalan y cómo encajan el proxy, los flujos y el hub.
---

## Proxy local

Jevalan expone un daemon HTTP en la máquina local. El agente llama a Jevalan en vez de conectarse directamente al backend. El proxy puede añadir la clave configurada y reenvía la petición System One.

## Tres rutas de decisión

| Ruta | Para qué sirve |
|---|---|
| `POST /v1/systemone` | Enviar una petición System One al backend predeterminado. |
| `POST /v1/decide/{flujo}` | Ejecutar un flujo YAML por nombre. |
| `POST /v1/hub` | Resolver una capacidad tipada, como `tool.risk`. |

Los flujos y las capacidades se cargan desde el directorio de trabajo y se pueden observar desde la TUI. La ruta `/events` publica eventos para la interfaz.

## Quién ejecuta la acción

Jevalan devuelve una respuesta o una decisión. No ejecuta la acción solicitada, no cambia archivos del agente ni aprueba operaciones en su nombre. El cliente conserva esa responsabilidad.

## Backends y proveedores

Un flujo puede usar reglas locales, consultar un backend compatible o pedir una decisión a un proveedor configurado. El hub también puede usar un proveedor `flow`, `http` o `exec`; la ejecución de procesos está desactivada por defecto.
