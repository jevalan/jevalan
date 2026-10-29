---
title: Configuración
description: Configura el daemon, sus backends y las rutas de flujos y capacidades.
---

Empieza con `config.example.yaml`. Cópialo a `config.yaml` y mantén la configuración junto a `config/flows/`, `config/capabilities/`, `config/signals/` y `config/lab/`.

```yaml
server:
  addr: "127.0.0.1:8080"
backends:
  ollama:
    url: "http://localhost:11434"
default_backend: ollama
flows_dir: ./config/flows
hub:
  capabilities_dir: ./config/capabilities
  signals_dir: ./config/signals
  allow_exec: false
```

Las rutas relativas se resuelven desde el directorio de trabajo actual. Ejecuta Jevalan desde la raíz del paquete extraído para que encuentre los recursos incluidos.

## Ajustes

La precedencia es: flags de la línea de comandos, luego variables de entorno `JEVALAN_*`, luego el fichero y por último los valores por defecto. Jevalan también lee al arrancar un fichero `.env` del directorio de trabajo; nunca pisa una variable que ya esté definida.

| Ajuste | Clave en el fichero | Variable de entorno | Por defecto |
|---|---|---|---|
| Dirección de escucha | `server.addr` | `JEVALAN_ADDR` | `127.0.0.1:8080` |
| Nombre de la variable del token | `server.token_env` | (la variable que nombra) | `JEVALAN_TOKEN` |
| Backends | `backends.<nombre>` | ver abajo | solo `ollama` |
| Backend por defecto | `default_backend` | `JEVALAN_DEFAULT_BACKEND` | `ollama` |
| Directorio de flujos | `flows_dir` | `JEVALAN_FLOWS_DIR` | `./config/flows` |
| Directorio de capacidades | `hub.capabilities_dir` | `JEVALAN_CAPABILITIES_DIR` | `./config/capabilities` |
| Directorio de señales | `hub.signals_dir` | `JEVALAN_SIGNALS_DIR` | `./config/signals` |
| Permitir proveedores `exec` | `hub.allow_exec` | `JEVALAN_ALLOW_EXEC` | `false` |
| Fichero de configuración | `--config` | `JEVALAN_CONFIG` | ninguno |

## Backends

Cada entrada de `backends` es un servicio System One al que el daemon puede reenviar:

```yaml
backends:
  typesafe:   { url: "https://api.typesafe.ai", api_key_env: TYPESAFE_API_KEY, timeout_ms: 2000 }
  openrouter: { url: "https://openrouter.ai", path: "/api/alpha/decisions", api_key_env: OPENROUTER_API_KEY, timeout_ms: 8000 }
  ollama:     { url: "http://localhost:11434", timeout_ms: 5000 }
```

| Clave | Significado |
|---|---|
| `url` | Obligatoria. Debe ser `http` o `https`. |
| `api_key_env` | El **nombre** de la variable de entorno que guarda la clave. Se envía como `Authorization: Bearer <clave>`. |
| `timeout_ms` | Plazo de una llamada. Por defecto `5000`. |
| `path` | Ruta que se añade a `url`. Por defecto `/v1/systemone`. Debe empezar por `/` y no puede contener `..`. |

Los nombres de backend usan minúsculas, dígitos y guiones, y no pueden ser `v1`. La ruta que llaman tus clientes, `/{backend}/v1/systemone`, nunca cambia, sea cual sea el `path`.

**Ollama como backend.** A partir de la versión 0.35.0, Ollama sirve modelos de decisión por este mismo contrato (por ejemplo, `ollama pull nimble`) y se llaman por el nombre del modelo. Con una versión anterior, o sin un modelo de decisión descargado, `/v1/systemone` no responde como System One (en una 0.32.15 devuelve `404`) y todos los nodos salen por `error`. Un modelo de chat corriente de Ollama no sirve: no devuelve probabilidades propias.

El daemon falla enseguida, con un mensaje claro, si `default_backend` no está definido, una `url` no es http ni https, o una variable de `api_key_env` está vacía o sin definir.

Los flujos nombran el backend que usan. Si un flujo nombra un backend que tu configuración no define, ese flujo aparece como inválido hasta que lo añadas. Mira [Solución de problemas](/solucion-de-problemas/).

## Claves y acceso

Guarda las claves en variables de entorno y pon el nombre de cada variable en el YAML con `api_key_env`. No pongas claves reales en un fichero versionado.

Si defines la variable del token (`JEVALAN_TOKEN` por defecto), todas las rutas salvo `/healthz` exigen `Authorization: Bearer <token>`. Usa un token siempre que otros usuarios o procesos puedan llegar a la dirección en la que escucha el daemon.

Deja `allow_exec: false` salvo que quieras de forma explícita que el hub lance procesos locales. Mira [Capacidades y hub](/capacidades/).

Consulta [`config.example.yaml`](https://github.com/jevalan/jevalan/blob/main/config.example.yaml) para ver la estructura completa y valores de ejemplo.

## Directorios vigilados

El daemon vigila `flows_dir`, `capabilities_dir` y `signals_dir`. Guardar un fichero válido sustituye su contenido sin reiniciar. Un fichero inválido conserva la versión anterior y se informa con su motivo. Borrar un fichero deja de servirlo.
