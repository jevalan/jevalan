---
title: Servidor MCP
description: Deja que cualquier cliente MCP pida decisiones a Jevalan y liste, valide y pruebe flujos con un solo comando.
---

`jevalan mcp` es un puente pequeño entre el [Model Context Protocol](https://modelcontextprotocol.io) y la API HTTP del daemon. Un cliente MCP como Claude Code lo arranca por stdio, y cada capacidad que el hub tiene cargada aparece como una herramienta. No añade lógica de decisión propia: cada llamada se reenvía al daemon, que sigue aplicando sus plazos, su validación y sus eventos.

## Conectar un cliente

El daemon tiene que estar corriendo (`jevalan serve`). El puente lo encuentra igual que los hooks: por el fichero de descubrimiento (`daemon.json`) o, si no hay, por la dirección por defecto.

Para Claude Code, añádelo al `.mcp.json` de tu proyecto, con la **ruta absoluta** a una copia del binario:

```json
{
  "mcpServers": {
    "jevalan": { "command": "/path/to/jevalan", "args": ["mcp"] }
  }
}
```

| Opción | Significado |
|---|---|
| `--addr host:puerto` | Habla solo con este daemon e ignora `daemon.json`. |
| `--token` | El token del daemon (también se lee de `JEVALAN_TOKEN`). Nunca se imprime. |
| `--allow-live` | Deja que `jevalan_try_draft` use los transportes `live` y `record`. Mira más abajo. |

## Herramientas

**Una por capacidad.** Cada capacidad cargada se convierte en una herramienta cuyo esquema de entrada es el de la propia capacidad. El nombre es el id de la capacidad con todo carácter que no sea letra, cifra, `_` o `-` sustituido por `_`; así `tool.risk` es `tool_risk` y `agent.wakeup` es `agent_wakeup`. El resultado es la respuesta tipada del hub: `status` vale `ok`, `abstain` o `error`, y solo `error` se marca como llamada fallida. Cada 5 segundos el puente comprueba si la lista cambió (una capacidad que aparece, desaparece o cambia el esquema de su entrada) y, si es así, avisa al cliente. Un cambio solo de descripción no avisa.

**Cuatro para flujos**, siempre presentes:

| Herramienta | Qué hace |
|---|---|
| `jevalan_run_flow` | Ejecuta un flujo publicado por su nombre con un `state` y devuelve su `outcome`, `data` y `path`. |
| `jevalan_list_flows` | Lista los flujos cargados y los inválidos, con el motivo. |
| `jevalan_validate_flow` | Valida el YAML de un flujo con el analizador real, sin publicarlo. Un flujo inválido responde `ok: false` con el problema; es un veredicto, no una llamada fallida. |
| `jevalan_try_draft` | Ejecuta un borrador una vez, sin publicarlo. |

## Lo que no hace, a propósito

- No puede guardar, editar ni borrar flujos, ni tocar `config/signals/`. Cambiar un flujo sigue siendo cosa de una persona, con [`jevalan edit`](/comandos/). `tool-risk-local` decide de verdad cuando el hook de Claude Code está instalado, y un agente reescribiendo sus propias reglas de seguridad echaría por tierra el sentido de todo.
- No lanza procesos y no abre ningún puerto de red propio.
- Si el daemon no está o deja de contestar, la llamada vuelve como error con el motivo. Nunca se inventa un veredicto.

## Sobre `--allow-live`

`jevalan_try_draft` recibe un `transport`. Por defecto solo se ofrece `replay`: reproduce respuestas grabadas y no llama a nada. Sin `cassette` no hay nada que reproducir, y el flujo sigue su camino de fallo en vez de devolver un error de la herramienta. `live` y `record` llaman a tus backends reales, lo que puede costar dinero, así que se rechazan salvo que arranques el puente con `--allow-live`.

## Preguntar no es lo mismo que obligar

Un agente ya puede consultar una llamada con `tool_risk` antes de hacerla. Es una comodidad, no una barrera: un modelo al que se le pide por favor también puede decidir no preguntar. Para imponer una decisión, sigue usando el [hook](/integraciones/).
