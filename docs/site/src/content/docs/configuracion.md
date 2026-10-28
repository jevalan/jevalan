---
title: Configuración
description: Configura el daemon, sus backends y las rutas de flujos y capacidades.
---

Empieza con `config.example.yaml`. Copia el archivo a `config.yaml` y conserva juntos la configuración, `config/flows/`, `config/capabilities/`, `config/signals/` y `config/lab/`.

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

Las rutas relativas se resuelven desde el directorio actual. Ejecuta Jevalan desde la raíz extraída del paquete para que apunte a los recursos incluidos.

## Claves y acceso

Guarda las claves en variables de entorno y configura en YAML el nombre de cada variable (`api_key_env`). No pongas claves reales en un archivo versionado. Si configuras `token_env`, las llamadas al daemon necesitan enviar el token.

Mantén `allow_exec: false` salvo que quieras permitir explícitamente que el hub lance procesos locales. Usa un token si otros usuarios o procesos pueden alcanzar la dirección donde escucha el daemon.

Consulta [`config.example.yaml`](https://github.com/jevalan/jevalan/blob/main/config.example.yaml) para ver la estructura completa y sus valores de ejemplo.
