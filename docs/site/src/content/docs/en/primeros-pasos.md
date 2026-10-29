---
title: Getting started
description: Install Jevalan, run the demo, and start the local daemon.
---

Jevalan is a terminal program and a local daemon. This website is only the project presentation and user guide; requests are processed on your machine.

## Download

When a release is available, download the package for your system from [GitHub Releases](https://github.com/jevalan/jevalan/releases). Extract the complete archive and run commands from that directory: it contains the binary, flows, capabilities, and example configuration.

If there is no release yet, clone the repository and install Rust 1.89 or later to build it:

```sh
git clone https://github.com/jevalan/jevalan.git
cd jevalan
cargo build --locked --release -p jevalan
```

## Try the demo

The demo includes the daemon, mock backends, and simulated traffic. No credentials are required:

```sh
# Windows PowerShell
.\target\release\jevalan.exe tui --demo

# Linux and macOS
./target/release/jevalan tui --demo
```

![Terminal screenshot of `jevalan tui --demo`: clients on the left, the hub with its capabilities at the top, the backends with their latency at the bottom, and the live decisions feed.](../../../assets/tui-topologia.png)

## Start with your backends

From the extracted directory, copy the example configuration and review its endpoints:

```powershell
Copy-Item config.example.yaml config.yaml
```

Set the relevant backend keys as environment variables, then start the daemon:

```sh
jevalan serve --config config.yaml
```

Open another terminal in the same directory and run `jevalan tui` to see the status and events. The daemon does not generate requests by itself; the client sends them.

The example file points to external providers and local Ollama. The demo is the only mode in this guide that guarantees it will not call real services.
