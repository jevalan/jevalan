---
title: Arquitectura
description: Cómo encajan el proxy, el motor de flujos, el hub, el bus de eventos y el laboratorio.
---

Jevalan es un daemon local con tres puertas de entrada, un motor en medio y un flujo de eventos que vigila la interfaz de terminal.

```text
        agentes, hooks, scripts
                │
   ┌────────────┼──────────────────────────┐
   │            │                          │
POST /{backend}/v1/systemone   POST /v1/decide/{flujo}   POST /v1/hub
(pregunta System One cruda)    (un flujo concreto)       (por capacidad)
   │            │                          │
┌──▼────────────▼──────────────────────────▼─────────────────────┐
│                           JEVALAN                              │
│                                                                │
│  passthrough ─────────┐   motor de flujos ◄── registro de      │
│  (reenvía sin tocar)  │   rule · noul ·        capacidades     │
│                       │   choice · score ·     ├─ flow → tus flujos
│                       │   batch                ├─ http → un servicio
│                       ▼          │             └─ exec → un proceso
│           Backends System One (OpenRouter, TypeSafe, Ollama,   │
│           modelos locales): claves, plazos, coste              │
│                                                                │
│  bus de eventos ──── WebSocket /events ────► interfaz de terminal
└────────────────────────────────────────────────────────────────┘
                          ▲
                          │ mismo camino, en proceso, sin HTTP
                  jevalan lab (offline · record · live)
```

## Tres principios

1. **El modelo juzga, tu código actúa.** Un modelo devuelve probabilidades tipadas. Reglas y umbrales locales, que puedes leer y probar, las convierten en una decisión. El agente aplica el efecto. Jevalan nunca lo ejecuta.
2. **El agente no sabe quién decide.** Pide una capacidad. Que la responda un flujo tuyo o un especialista ajeno es una elección de configuración, así que puedes cambiarla sin tocar el agente.
3. **La telemetría es explícita.** Cada paso emite un evento y lo ves en la TUI. Los eventos no tienen un campo para la entrada original, pero pueden llevar respuestas, resultados, señales y errores, así que trata el flujo como sensible.

## Las piezas

| Pieza | Función |
|---|---|
| **Passthrough** | Reenvía una petición System One a un backend sin tocarla y guarda la clave, el plazo y la cuenta del coste. |
| **Motor de flujos** | Recorre un grafo YAML de reglas y preguntas a un modelo y devuelve un resultado con el camino que siguió. |
| **Registro de capacidades** | Asigna una pregunta tipada a un proveedor: un flujo, un servicio HTTP o un proceso local. |
| **Bus de eventos** | Publica lo que ocurre, nodo a nodo, por un WebSocket. |
| **Interfaz de terminal** | Muestra en vivo la topología, las métricas, las decisiones y las trazas, y edita flujos como borradores. |
| **Laboratorio** | Pasa los mismos casos por el camino del hub, sin conexión o contra backends reales. |
| **Adaptadores de hook** | Dejan que Claude Code y Crush consulten `tool.risk` antes de cada llamada a una herramienta. |

## Una decisión, de punta a punta

Imagina una sesión de Claude Code a punto de ejecutar `rm -rf /tmp/build`:

1. Claude Code ejecuta el hook `PreToolUse`, que lee la llamada a la herramienta por stdin.
2. El hook encuentra el daemon por su fichero de descubrimiento y envía `POST /v1/hub` para `tool.risk`.
3. El hub resuelve `tool.risk` al flujo `tool-risk-local`.
4. Primero corren dos reglas fijas: un borrado recursivo de la raíz o del hogar se deniega, y una descarga canalizada a un intérprete pregunta. Ninguna llama a un modelo.
5. Si no casa ninguna, un nodo `batch` hace cuatro preguntas de riesgo en una sola llamada a un modelo local y obtiene cuatro probabilidades.
6. Las reglas de enrutado aplican los umbrales: cualquier riesgo de 0.85 o más es `deny`, de 0.55 o más es `ask`, y si no, `allow`.
7. El hub valida la decisión contra su esquema y la devuelve. Cada paso se publicó como evento.
8. El hook anota la decisión en el registro sombra. En modo `enforce` también responde a Claude Code; en `shadow` calla.

El hook solo puede endurecer los permisos de Claude Code, nunca relajarlos. En el peor caso, pregunta de más.

## Para profundizar

- [Flujos](/flujos/) y [Capacidades y hub](/capacidades/) explican las dos formas de definir una decisión.
- [Laboratorio](/laboratorio/) explica cómo medir una.
- El repositorio tiene el diseño completo con diagramas más detallados: [`docs/es/architecture.md`](https://github.com/jevalan/jevalan/blob/main/docs/es/architecture.md).
