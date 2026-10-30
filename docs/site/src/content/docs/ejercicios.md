---
title: Ejercicios
description: Cinco ejercicios cortos, sin red ni claves, para aprender a fijar con fixtures lo que deciden tus reglas.
---

Estos ejercicios usan el [laboratorio](/laboratorio/) en modo `offline`: cada caso lleva la respuesta del modelo escrita a mano (`replay`), así que prueban **tus reglas y tus umbrales**, no ningún modelo, y dan siempre el mismo resultado. No hace falta daemon ni claves. Haz primero la [guía de decisiones](/guia-decisiones/).

Trabaja sobre una copia para no tocar `config/lab`:

```sh
cp -r config/lab mi-lab          # PowerShell: Copy-Item -Recurse config\lab mi-lab
```

## 1. Mira lo que ya está fijado

```sh
jevalan lab check --lab-dir mi-lab
jevalan lab offline --lab-dir mi-lab
```

`check` valida los ficheros y `offline` los reproduce. Verás una tabla por capacidad con `pasa`, `skipped` y `fallan`. Abre `mi-lab/fixtures/agent.wakeup/judged-sleep.yaml`: una entrada, la respuesta del modelo (`worth: { answer: 0.08, … }`) y dos aserciones con su `why`.

**Pregunta:** ¿por qué ese caso duerme al agente y `ambiguous-warning` (con `answer: 0.65`) lo despierta? Mira los umbrales de `config/flows/agent-wakeup.yaml`.

## 2. Escribe tu primer fixture

Crea `mi-lab/fixtures/agent.wakeup/disk-space-sleep.yaml`:

```yaml
id: disk-space-sleep
version: 1
description: "Un aviso de disco mientras se espera una build no despierta al agente"
input: { waiting_for: "una build terminada", event: "aviso: el disco D: está al 71 %", skipped: 1 }
replay:
  worth: { answer: 0.12, confidence: 0.9 }
expect:
  - cel: "status == 'ok' && decision.wake == false"
    why: "Un aviso de disco no tiene que ver con la build"
```

El `id` es igual que el nombre del fichero. Ejecuta `check` y `offline`: verás `lab en orden: 20 fixtures` y `agent.wakeup` pasa de 11 a 12 casos que pasan.

## 3. Haz que falle a propósito

Cambia la aserción a `decision.wake == true` y vuelve a ejecutar `offline`:

```text
Falla `disk-space-sleep` (status ok):
- Un aviso de disco no tiene que ver con la build
```

El `why` es lo que se imprime. Por eso conviene escribirlo como **la regla que proteges**, no como una descripción del test. Vuelve a dejar `wake == false`.

## 4. Predice un borde

Cambia el `replay` a `answer: 0.4999`. Antes de ejecutar, escribe en la aserción lo que crees que pasa: ¿duerme? ¿despierta? ¿con qué `reason`? Los intervalos del nodo `worth` son `0..0.3`, `0.3..0.5` y `0.5..1`, semiabiertos por la derecha.

```yaml
expect:
  - cel: "status == 'ok' && decision.wake == true && decision.reason == 'unsure'"
    why: "Justo por debajo de 0.5 la señal es dudosa y debe despertar"
```

Con ese cambio el fixture pasa: 0.4999 sigue en la zona dudosa (`unsure`) y despierta. Compáralo con `threshold-edge-judged`, que fija el valor 0.5. Si te equivocas en la predicción, has aprendido algo sobre tu umbral antes de que lo aprenda un usuario.

## 5. De tráfico real a fixtures

El modo `shadow` del [hook](/integraciones/) guarda lo que Jevalan habría decidido. Conviértelo en casos:

```sh
jevalan shadow fixtures --lab-dir mi-lab --max 20
```

Escribe un fixture por decisión (las de más riesgo primero), con las cuatro probabilidades grabadas como `replay` y la decisión como aserción:

```yaml
replay:
  risks.destructive:
    answer: 0.62
    confidence: 0.04
  # …
expect:
  - cel: decision.verdict == 'ask'
    why: El veredicto grabado (ask) debe reproducirse
```

(Ejemplo con un registro de prueba.) Desde ahora, si cambias un umbral o una regla, `jevalan lab offline` te dice qué decisiones reales cambiarían. Este fixture guarda un `command` del registro: revisa lo que importa antes de subirlo a ningún repositorio.

## Qué has aprendido

- Un `replay` fija la respuesta del modelo para probar solo tus reglas.
- El `why` documenta la regla; el borde de un umbral se fija con un fixture.
- Los casos reales entran por `shadow fixtures`, y desde ahí la CI avisa si algo cambia.

Siguiente: [Laboratorio](/laboratorio/) para medir modelos reales (`record` y `live`) y [Catálogo de capacidades](/catalogo/) para ver qué más puedes fijar.
