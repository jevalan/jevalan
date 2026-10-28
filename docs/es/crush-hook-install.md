# Instalación del hook de Crush en esta máquina (Windows)

*[English version](../crush-hook-install.md)*

Cómo se monta el hook `PreToolUse` para que Crush pase por jevalan, en modo sombra y
sin frenar nunca. Es el mismo papel que el hook de Claude Code (`docs/es/claude-code-hook-install.md`),
pero por el subcomando propio `jevalan hook crush`: firma cada petición como `crush/<proyecto>:<sesión>`
—para que en la TUI no aparezca como `unknown`— y responde en la envoltura nativa de Crush.

## Por qué encaja sin puente

Los hooks de Crush son compatibles con los de Claude Code, y su contrato ya coincide con lo
que jevalan necesita:

- **Entrada (stdin JSON):** `{event, session_id, cwd, tool_name, tool_input}` — justo los campos
  que `extract_input` lee.
- **Salida:** Crush entiende `{"decision":"deny","reason":"…"}`, que es lo que emite el modo
  `enforce`. Crush solo distingue `allow` de `deny`; no sabe pedir confirmación desde un hook,
  así que en Crush el modo activo **bloquea o deja decidir a sus permisos**, nunca fuerza un
  diálogo. El hook jamás emite `allow`: solo puede endurecer.

## Piezas

| Pieza | Dónde |
|---|---|
| Binario estable (lo usan hook y daemon) | `%LOCALAPPDATA%\jevalan\bin\jevalan.exe` |
| Hook global | `%USERPROFILE%\.config\crush\crushrc` → `hook add PreToolUse …` |
| Daemon | tarea programada `jevalan-daemon` (al iniciar sesión), cwd = repo, `serve --config config/lab/config.openrouter.yaml` |
| Salida del daemon | `%LOCALAPPDATA%\jevalan\serve.log` |
| Descubrimiento (addr + token) | `%LOCALAPPDATA%\jevalan\daemon.json` |
| Registro sombra | `%LOCALAPPDATA%\jevalan\shadow.jsonl` |

El hook apunta a una **copia** del binario, nunca a `target/`: un `cargo clean` o una compilación
a medias no deben romper todas las sesiones de Crush. Para ponerla al día, `cargo build --release -p jevalan`
y `./tools/scripts/update-installed-binary.ps1`.

## Configurar el hook

En el `crushrc` global (`%USERPROFILE%\.config\crush\crushrc`), una línea:

```bash
hook add PreToolUse --name jevalan --matcher '^(bash|edit|write|multiedit)$' --command '"$LOCALAPPDATA/jevalan/bin/jevalan.exe" hook run --client crush --dialect crush --mode shadow' --timeout 8
```

- El subcomando es genérico: `hook run --client <nombre> --dialect <claude|crush>`. Añadir un
  harness nuevo no añade código a jevalan, solo cambia flags; `hook crush` es un alias de
  `hook run --client crush --dialect crush`.
- El `--matcher` deja fuera las herramientas que no ejecutan nada ni tocan el disco (`view`, `ls`,
  `grep`, `glob`, `question`, `todos`, …): el hook Rust consulta por toda herramienta con `tool_name`,
  así que el filtro vive aquí, no en jevalan.
- `--command` es una línea de shell que Crush ejecuta con su shell embebido; `$LOCALAPPDATA` se
  expande al lanzarla. Si prefieres ruta absoluta o `crush.json`, el equivalente es:

  ```jsonc
  {
    "hooks": {
      "PreToolUse": [
        {
          "name": "jevalan",
          "matcher": "^(bash|edit|write|multiedit)$",
          "command": "\"%LOCALAPPDATA%\\jevalan\\bin\\jevalan.exe\" hook run --client crush --dialect crush --mode shadow",
          "timeout": 8
        }
      ]
    }
  }
  ```

Cambia `--mode shadow` por `--mode enforce` para que un veredicto `deny` bloquee la herramienta.
En `enforce`, `ask` y las indecisiones dejan pasar: el hook no puede pedir confirmación.

Los hooks se releen al arrancar Crush; para aplicar el cambio, reinicia la sesión.

## Comprobar

Con el daemon levantado, una herramienta (por ejemplo `bash`) debe aparecer en la TUI como
`crush/<proyecto>:<sesión>`, y cada decisión queda en `shadow.jsonl`. Sin daemon, el hook apunta
`"sin daemon"` y sale sin estorbar.

## Desinstalar

Quita la entrada `jevalan` del `crushrc` (`hook remove PreToolUse --name jevalan`) o del bloque
`hooks.PreToolUse` de `crush.json`.

## Privacidad: solo modelos locales (regla vigente)

El hook es **global**: cada herramienta de **cualquier** proyecto de la máquina se envía al daemon.
Por eso rige la misma regla que para el hook de Claude Code (véase `docs/local-development.md`):

- El daemon del hook usa **`config/lab/config.local.yaml`**: `tool.risk` lo decide `config/flows/tool-risk-local.yaml`
  contra **Kev (`127.0.0.1:8009`) o Laya (`127.0.0.1:8021`)**. Nada sale de la máquina.
- **Prohibido** apuntar ese daemon a un backend o especialista en la nube mientras no exista el
  blindaje de salida.
- `shadow.jsonl` guarda los comandos completos (con lo reconocible tachado): no se sube a ningún
  repo ni se comparte. Lo publicable es `jevalan shadow report`.
