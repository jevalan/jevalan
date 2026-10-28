---
title: Primeros pasos
description: Instala Jevalan, ejecuta la demo y arranca el daemon local.
---

Jevalan es un programa de terminal y un daemon local. Esta web es solo la presentación y la ayuda; las peticiones se procesan en tu equipo.

## Descargar

Cuando haya una versión publicada, descarga el paquete de tu sistema desde [GitHub Releases](https://github.com/jevalan/jevalan/releases). Extrae el archivo completo y ejecuta los comandos desde esa carpeta: contiene el binario, los flujos, las capacidades y la configuración de ejemplo.

Si todavía no hay una release, clona el repositorio e instala Rust 1.89 o posterior para compilar:

```sh
git clone https://github.com/jevalan/jevalan.git
cd jevalan
cargo build --locked --release -p jevalan
```

## Ver la demo

La demo integra el daemon, backends falsos y tráfico simulado. No necesita configurar credenciales:

```sh
# Windows PowerShell
.\target\release\jevalan.exe tui --demo

# Linux y macOS
./target/release/jevalan tui --demo
```

## Arrancar con tus backends

Desde la carpeta extraída, copia la configuración de ejemplo y revisa sus endpoints:

```powershell
Copy-Item config.example.yaml config.yaml
```

Configura como variables de entorno las claves que correspondan a los backends que vayas a usar. Después inicia el daemon:

```sh
jevalan serve --config config.yaml
```

Abre otra terminal en la misma carpeta y ejecuta `jevalan tui` para ver el estado y los eventos. El daemon no genera peticiones por sí mismo: las envía el cliente que lo consulta.

El archivo de ejemplo apunta a proveedores externos y Ollama local. La demo es el único modo de esta guía que garantiza no llamar a servicios reales.
