---
title: Integraciones
description: Conecta Jevalan con Claude Code y Crush mediante hooks, y aprende de lo que registran.
---

Jevalan incluye adaptadores de hook. Un hook es un programa pequeño que el cliente ejecuta **antes de cada llamada a una herramienta**: envía la llamada al hub local, pide la capacidad `tool.risk` y traduce la respuesta al formato que espera el cliente. Jevalan no ejecuta nada; la última palabra la tiene siempre el cliente.

## Shadow y enforce

Todo hook corre en uno de dos modos, que se elige con `--mode`:

| | `shadow` (por defecto) | `enforce` |
|---|---|---|
| Qué imprime al cliente | Nada, y siempre sale con 0 | Un veredicto cuando es más estricto que el del propio cliente |
| Efecto en la sesión | Ninguno: el cliente sigue exactamente igual | `deny` bloquea, `ask` hace que el cliente te pregunte |
| Registra la decisión | Sí, en `shadow.jsonl` | Sí |

En `enforce`, el hook solo puede **endurecer** los permisos, nunca relajarlos:

| Veredicto | Qué hace el hook |
|---|---|
| `deny` | Bloquea la herramienta y muestra el motivo. |
| `ask` | Hace que el cliente te pregunte. |
| `allow` | Calla: los permisos del propio cliente siguen aplicando. |
| sin respuesta (sin daemon, abstención, error, plazo, entrada mala) | Responde `ask`, nunca un `allow` a ciegas. |

Empieza en `shadow`, lee el informe unos días y solo entonces pasa a `enforce`.

## Claude Code

Añade el hook a `~/.claude/settings.json`, con la **ruta absoluta** a una copia del binario (nunca apuntes a `target/`, así un `cargo clean` no rompe tus sesiones):

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "*",
        "hooks": [
          { "type": "command", "command": "/ruta/a/jevalan hook claude-code --mode shadow" }
        ]
      }
    ]
  }
}
```

Arranca el daemon con `jevalan serve`. Cuando escucha, escribe un fichero de descubrimiento (`daemon.json`, con la dirección y el token) en `%LOCALAPPDATA%/jevalan` en Windows o en `$HOME/.jevalan` en el resto, y el hook lo encuentra desde cualquier carpeta. Si el daemon no está corriendo, el hook no encuentra el fichero y ni siquiera intenta conectar.

La guía completa, con cómo actualizar el binario instalado en Windows, está en el repositorio: [Instalar el hook de Claude Code](https://github.com/jevalan/jevalan/blob/main/docs/es/claude-code-hook-install.md).

## Crush

Los hooks de Crush son compatibles con los de Claude Code. En tu `crushrc`, una línea:

```sh
hook add PreToolUse --name jevalan --matcher '^(bash|edit|write|multiedit)$' --command '"$LOCALAPPDATA/jevalan/bin/jevalan.exe" hook run --client crush --dialect crush --mode shadow' --timeout 8
```

`jevalan hook run --client <nombre> --dialect <claude|crush>` es genérico: dar soporte a otro cliente cambia flags, no código. Crush solo distingue `allow` de `deny`, así que en `enforce` bloquea o deja decidir a sus propios permisos. Mira [Instalar el hook de Crush](https://github.com/jevalan/jevalan/blob/main/docs/es/crush-hook-install.md).

## Privacidad: usa modelos locales

Un hook es **global**: cada llamada a una herramienta de cada proyecto de tu máquina, incluidos repositorios de clientes y ficheros `.env`, va al daemon, y el daemon se la pasa a quien sirva `tool.risk`. La redacción del registro sombra solo protege el fichero local; **lo que el daemon reenvía sale tal cual**.

Por eso, mientras no exista un blindaje de salida:

- Sirve `tool.risk` con un modelo **local**, como hace `config/flows/tool-risk-local.yaml`. Nada sale de tu máquina.
- **No** apuntes el daemon del hook a un backend en la nube ni a un especialista de terceros.

El blindaje que haría falta es: no consultar cuando la herramienta toca rutas sensibles (`.env*`, `*.pem`, `id_rsa*`, kubeconfig…), redactar también la entrada saliente, sanear la transcripción, permitir solo carpetas de una lista y validar los patrones de redacción contra tus propios repositorios.

## Qué registra el hook

Cada decisión se añade a `shadow.jsonl` en el mismo directorio de datos, con los secretos de formas conocidas (`sk-…`, `ghp_…`, `AKIA…`, `Bearer …`, `clave=valor`) sustituidos por `[REDACTED]`. El fichero se queda en tu máquina, y aun así contiene los comandos, así que no lo subas a ningún sitio.

Conviértelo en algo legible, y luego en tests:

```sh
# Un informe en Markdown: veredictos, por qué hubo asks, probabilidades, latencia
jevalan shadow report --since 2026-10-01 --out informe.md

# Lo mismo, con un extracto redactado de cada ejemplo (solo para uso local)
jevalan shadow report --with-inputs

# Decisiones reales como fixtures del laboratorio, las de más riesgo primero
jevalan shadow fixtures --lab-dir lab-fixtures-batch --max 20
```

Por defecto el informe no contiene la entrada de la herramienta, así que puedes publicarlo. Los fixtures alimentan el [laboratorio](/laboratorio/), de modo que tu banco de pruebas crece a partir de lo que de verdad pasó.

En la interfaz de terminal, pulsa `H` para ver las decisiones en vivo: contadores con latencia p50 y p95, las últimas llamadas y la probabilidad de cada pregunta como barras con marcas en los umbrales 0.55 y 0.85. Lee el fichero en local y nunca muestra la entrada, así que es seguro hacer capturas.

![Captura de la terminal con la vista de decisiones del hook: contadores de veredictos, lista de decisiones y la probabilidad de cada pregunta en barras.](../../assets/tui-decisiones-hook.png)

## Más allá de los hooks

Si tu cliente habla MCP, [`jevalan mcp`](/servidor-mcp/) expone las capacidades como herramientas, sin escribir un adaptador.

## Desinstalar

Quita la entrada `jevalan` de `PreToolUse` en `~/.claude/settings.json` (o desactívala desde `/hooks`), o ejecuta `hook remove PreToolUse --name jevalan` en Crush.
