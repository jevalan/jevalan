---
title: Solución de problemas
description: Arregla los problemas más comunes con el daemon, los flujos, los hooks y el laboratorio.
---

## Un flujo o una capacidad aparece como «inválido»

Al arrancar, el daemon imprime una línea por cada flujo y capacidad, y `GET /v1/flows` y `GET /v1/hub/capabilities` listan los inválidos en `invalid`, con el motivo:

```text
flujo cargado: ticket-triage (2 nodos)
flujo inválido: agent-wakeup: worth: backend desconocido "openrouter"
capacidad inválida: agent.wakeup: proveedor inválido: flujo desconocido: agent-wakeup
```

La causa habitual es un flujo que nombra un backend que tu configuración no define (`backend desconocido`). Añade el backend a `backends:` en tu config, con su clave si la necesita. Una capacidad servida por un flujo pasa a inválida mientras ese flujo lo sea, así que arreglar el flujo arregla las dos cosas.

Esto es lo esperado con las configuraciones mínimas, que definen solo algunos backends. No es un fallo, pero los flujos que necesitan los backends que faltan no se servirán.

## Todos los nodos salen por `error` con un backend local

Comprueba el servidor **sin pasar por Jevalan** (mira [Backends compatibles](/backends/)). Si es Ollama y `POST /v1/systemone` da `404`, tu versión es anterior a la 0.35.0: actualiza y descarga un modelo de decisión (`ollama pull nimble`). Si responde `200` pero el nodo sigue saliendo por `error`, lee el mensaje en el `path` de la respuesta: lo habitual es que el servidor no implemente el tipo de pregunta del nodo.

## El daemon no arranca

- **Falta una variable.** El daemon falla enseguida si `default_backend` no está definido, una `url` no es http ni https, o un `api_key_env` nombra una variable vacía o sin definir. El mensaje empieza por `configuración inválida:`, dice cuál, y el daemon sale con el código `2`.
- **El puerto está ocupado.** El daemon sale con el código `1` y el error del sistema de «dirección ya en uso» (`os error 10048` en Windows). Cambia `server.addr`, o define `JEVALAN_ADDR`.
- **Rutas relativas.** `flows_dir`, `capabilities_dir` y `signals_dir` se resuelven desde la carpeta en la que arrancas el daemon. Arráncalo desde la raíz del proyecto, o usa rutas absolutas y define también `signals_dir`, o `tool-risk-local` no cargará. Un directorio que no existe no es un error: el daemon arranca, escribe un aviso como `aviso: no existe el directorio de flujos ./flows` y no sirve nada de él.

## La interfaz de terminal no muestra nada

La TUI es un visor: la actividad aparece solo cuando un cliente envía peticiones. Comprueba, por orden:

1. ¿Está corriendo el daemon? `curl http://127.0.0.1:8080/healthz` debería imprimir `ok`.
2. ¿El daemon exige token? Pasa `--token` o define `JEVALAN_TOKEN`. Si no lo pasas, la TUI toma el token del fichero de descubrimiento.
3. ¿Quieres verla funcionando sin nada más? Ejecuta `jevalan tui --demo`, que trae sus propios servicios y tráfico.

## Las peticiones dan `502`

El backend no responde, o un nodo falló y no tiene puerto `error`. Revisa la URL y la clave del backend, y añade un puerto `error` al nodo para que el flujo gestione el fallo a propósito en vez de terminar la petición.

## Un hook no hace nada

Para eso está el modo `shadow`: no imprime nada y siempre sale con 0. Para comprobar que funciona:

- Mira el registro sombra (`shadow.jsonl`, en el directorio de datos) o pulsa `H` en la TUI: ahí deberían aparecer las llamadas.
- Si no se registra nada, lo más probable es que el daemon no esté corriendo. El hook lee `daemon.json` para encontrarlo y, sin él, no intenta conectar.
- Un `daemon.json` que quedó de un daemon que se mató (en vez de pararlo con normalidad) apunta a un puerto que ya no responde. Arranca el daemon otra vez, o borra el fichero.
- En modo `enforce`, todo lo que el hub no puede decidir (sin daemon, abstención, error, plazo) responde `ask`, así que te preguntarán más a menudo mientras el daemon esté parado.

## Las capacidades `exec` responden `provider_disabled`

Lanzar procesos está desactivado por defecto. Pon `hub.allow_exec: true` (o `JEVALAN_ALLOW_EXEC=1`) si confías en la capacidad. Si en el comando se usa una variable como `${TOOLGATE_CLI}` y no está definida, la capacidad pasa a inválida cuando `allow_exec` está activado.

## `lab offline` falla

- **«no se ejecutó ningún fixture»** significa que todos los fixtures se filtraron o se saltaron. Revisa `modes` y si las cassettes están desactualizadas.
- **Un fixture sale `skipped` por una cassette desactualizada.** Cambiaste la `version` o el `input` del fixture. Ejecuta `jevalan lab record` otra vez para él.
- **`lab check` rechaza una clave de `replay`.** La clave debe ser un nodo del flujo o el `id` del proveedor.

## Compilar desde el código

- **Windows con Git Bash:** el `link.exe` que trae Git tapa el enlazador de MSVC y la compilación falla con un error del enlazador. Ejecuta Cargo con un `PATH` limpio solo para ese comando: `PATH="$(dirname "$(command -v cargo)"):/c/Windows/System32:/c/Windows" cargo build`.
- **Sin espacio en disco** (error 112 de Windows): deja la salida de Cargo en un volumen con espacio, con `CARGO_TARGET_DIR`.
- **Versión de Rust:** necesitas Rust 1.89 o posterior.

¿Sigues atascado? [Abre una incidencia](https://github.com/jevalan/jevalan/issues) con el comando que ejecutaste y la salida del daemon, y quita antes cualquier clave. Si es un problema de seguridad, sigue en cambio la [política de seguridad](https://github.com/jevalan/jevalan/blob/main/SECURITY.md).
