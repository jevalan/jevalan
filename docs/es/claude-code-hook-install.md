# Instalación del hook de Claude Code en esta máquina (Windows)

*[English version](../claude-code-hook-install.md)*

Cómo está montado el hook `PreToolUse` en modo sombra y cómo actualizarlo. El diseño está en `docs/es/architecture.md`, sección 6.

## Piezas instaladas

| Pieza | Dónde |
|---|---|
| Binario estable (lo usan hook y daemon) | `%LOCALAPPDATA%\jevalan\bin\jevalan.exe` |
| Hook global | `~/.claude/settings.json` → `PreToolUse`, matcher `*`, `jevalan.exe hook claude-code --mode shadow` |
| Daemon | tarea programada `jevalan-daemon` (al iniciar sesión), cwd = repo, `serve --config config/lab/config.openrouter.yaml` |
| Salida del daemon | `%LOCALAPPDATA%\jevalan\serve.log` |
| Descubrimiento (addr + token) | `%LOCALAPPDATA%\jevalan\daemon.json` |
| Registro sombra | `%LOCALAPPDATA%\jevalan\shadow.jsonl` |

El hook apunta a una **copia** del binario, nunca a `target/`: un `cargo clean` o una compilación a medias no deben romper todas las sesiones de Claude Code.

La TUI toma el token de `daemon.json` cuando no se le pasa `--token`, ni hay `JEVALAN_TOKEN` ni `.env`, así que `jevalan tui` funciona desde cualquier carpeta.

## Invocar `jevalan` desde cualquier terminal

El directorio del binario estable no está en el `PATH` por defecto. Para poder escribir `jevalan` a secas, añádelo al `PATH` **de usuario** (no hace falta admin, y así la consola y el daemon ejecutan la misma copia):

```powershell
$dir = Join-Path $env:LOCALAPPDATA 'jevalan\bin'
$cur = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($cur -split ';') -contains $dir) {
    "Ya estaba en el PATH de usuario."
} else {
    [Environment]::SetEnvironmentVariable('Path', ($cur.TrimEnd(';') + ';' + $dir), 'User')
    "Añadido: $dir"
}
```

Dos detalles que importan:

- **Se lee la variable de usuario, no `$env:PATH`.** `$env:PATH` es la vista mezclada de usuario y máquina; escribirla de vuelta en la de usuario incrustaría las rutas de máquina dentro de la tuya. La comprobación previa evita duplicar la entrada si se ejecuta dos veces.
- **Solo afecta a procesos nuevos.** Una terminal ya abierta conserva el `PATH` con el que arrancó, así que `Get-Command jevalan` falla ahí aunque el cambio esté hecho. Abre otra, o refresca la actual:

  ```powershell
  $env:PATH = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')
  ```

Comprueba en una terminal nueva:

```powershell
Get-Command jevalan
jevalan --version
```

El binario responde a `--version`.

El hook y la tarea programada siguen invocando la **ruta absoluta**, así que no se ven afectados por este cambio.

Como lo que queda en el `PATH` es la copia **estable**, `jevalan` ejecuta la versión instalada, que puede ir por detrás del código fuente — esa copia existe precisamente para que un `cargo clean` o una compilación a medias no rompan todas las sesiones de Claude Code. Para el fuente de ahora, `cargo run -p jevalan -- tui`; para poner al día la copia, `cargo build --release -p jevalan` y `./tools/scripts/update-installed-binary.ps1`.

## Sustituir el .exe tras recompilar

```powershell
cargo build --release -p jevalan
./tools/scripts/update-installed-binary.ps1
```

Hay que recompilar y reinstalar la copia estable (`cargo build --release -p jevalan` y `./tools/scripts/update-installed-binary.ps1`) siempre que `config/flows/` use algo que el binario instalado no conoce — por ejemplo `signals:`, desde la capa de señales. Un binario anterior conserva la versión del flujo que ya tenía cargada, pero tras un reinicio marca ese flujo como inválido y deja de servirlo.

### Si falla con «el proceso no tiene acceso al archivo porque está siendo utilizado por otro proceso»

Windows no deja sobrescribir un `.exe` que se está ejecutando, y la copia estable la usan **el daemon** (siempre) y **el hook** (un instante en cada herramienta). El script ya lo resuelve; a mano es:

```powershell
Stop-ScheduledTask jevalan-daemon                 # 1. parar el daemon
Get-Process jevalan | Select-Object Id, Path      # 2. ¿queda alguno? (p. ej. un serve lanzado a mano)
Copy-Item target\release\jevalan.exe "$env:LOCALAPPDATA\jevalan\bin\jevalan.exe" -Force
Start-ScheduledTask jevalan-daemon                # 3. arrancarlo de nuevo
```

Si la copia sigue fallando, es un hook en curso: espera un segundo y repite. Mientras el daemon está parado, el hook en modo sombra no molesta: apunta `"sin daemon"` y sale.

## Gestionar el daemon

```powershell
Start-ScheduledTask jevalan-daemon
Stop-ScheduledTask jevalan-daemon
Get-Content "$env:LOCALAPPDATA\jevalan\serve.log" -Tail 20
Unregister-ScheduledTask jevalan-daemon -Confirm:$false   # quitarlo del todo
```

## Desinstalar el hook

Quitar la entrada `jevalan.exe` de `PreToolUse` en `~/.claude/settings.json` (o desactivarla desde `/hooks`).

## Privacidad: solo modelos locales 

El hook es **global**: cada herramienta de **cualquier** proyecto de la máquina (también repos de clientes y repos personales con tokens, `.env`, secretos de Kubernetes…) se envía al daemon, y este la pasa a quien sirva `tool.risk`. La redacción del registro sombra solo protege el fichero local; **lo que el daemon reenvía va tal cual**.

Por eso:

- El daemon del hook usa **`config/lab/config.local.yaml`**: `tool.risk` lo decide `config/flows/tool-risk-local.yaml` contra **Kev (`127.0.0.1:8009`) o Laya (`127.0.0.1:8021`)**. Nada sale de la máquina.
- **Prohibido** apuntar ese daemon a un backend o especialista en la nube (Toolgate, Jev vía OpenRouter o TypeSafe) mientras no exista el blindaje de salida:
  1. no consultar en absoluto si la herramienta toca rutas sensibles (`.env*`, `*.pem`, `*.key`, `id_rsa*`, kubeconfig, `secret*.yaml`, `sealed*`, `credentials*`, `.npmrc`…), incluidos comandos que las nombren;
  2. redactar también la **entrada saliente**, no solo el registro, dejando marcas `[REDACTED:<tipo>]`;
  3. no pasar `transcript_path` tal cual: generar una transcripción temporal saneada;
  4. lista blanca de carpetas (`cwd`);
  5. patrones de redacción validados contra un escaneo de secretos de tus propios repositorios (tipo y fichero, nunca valores).
- `shadow.jsonl` es local pero guarda los comandos completos (con lo reconocible tachado): no se sube a ningún repo ni se comparte. Lo publicable es `jevalan shadow report`, que no incluye entradas.
