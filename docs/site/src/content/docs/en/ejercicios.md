---
title: Exercises
description: Five short exercises, with no network or keys, to learn how to pin with fixtures what your rules decide.
---

These exercises use the [lab](/en/laboratorio/) in `offline` mode: each case carries the model's answer written by hand (`replay`), so they test **your rules and thresholds**, not any model, and always give the same result. No daemon or keys are needed. Do the [decision guide](/en/guia-decisiones/) first.

Work on a copy so you do not touch `config/lab`:

```sh
cp -r config/lab my-lab          # PowerShell: Copy-Item -Recurse config\lab my-lab
```

## 1. Look at what is already pinned

```sh
jevalan lab check --lab-dir my-lab
jevalan lab offline --lab-dir my-lab
```

`check` validates the files and `offline` replays them. You will see a table per capability with `pasa` (passing), `skipped` and `fallan` (failing). Open `my-lab/fixtures/agent.wakeup/judged-sleep.yaml`: an input, the model's answer (`worth: { answer: 0.08, … }`) and two assertions, each with its `why`.

**Question:** why does that case put the agent to sleep while `ambiguous-warning` (with `answer: 0.65`) wakes it? Look at the thresholds in `config/flows/agent-wakeup.yaml`.

## 2. Write your first fixture

Create `my-lab/fixtures/agent.wakeup/disk-space-sleep.yaml`:

```yaml
id: disk-space-sleep
version: 1
description: "A disk warning while waiting for a build does not wake the agent"
input: { waiting_for: "a finished build", event: "warning: disk D: is 71% full", skipped: 1 }
replay:
  worth: { answer: 0.12, confidence: 0.9 }
expect:
  - cel: "status == 'ok' && decision.wake == false"
    why: "A disk warning has nothing to do with the build"
```

The `id` equals the file name. Run `check` and `offline`: you will see `lab en orden: 20 fixtures` and `agent.wakeup` goes from 11 to 12 passing cases.

## 3. Make it fail on purpose

Change the assertion to `decision.wake == true` and run `offline` again:

```text
Falla `disk-space-sleep` (status ok):
- A disk warning has nothing to do with the build
```

The `why` is what gets printed. That is why it should be written as **the rule you are protecting**, not as a description of the test. Put `wake == false` back.

## 4. Predict an edge

Change the `replay` to `answer: 0.4999`. Before running, write in the assertion what you think happens: does it sleep? wake? with which `reason`? The intervals of the `worth` node are `0..0.3`, `0.3..0.5` and `0.5..1`, half-open on the right.

```yaml
expect:
  - cel: "status == 'ok' && decision.wake == true && decision.reason == 'unsure'"
    why: "Just below 0.5 the signal is doubtful and must wake"
```

With that change the fixture passes: 0.4999 is still in the doubtful zone (`unsure`) and wakes. Compare it with `threshold-edge-judged`, which pins the value 0.5. If your prediction was wrong, you learned something about your threshold before a user did.

## 5. From real traffic to fixtures

The `shadow` mode of the [hook](/en/integraciones/) records what Jevalan would have decided. Turn it into cases:

```sh
jevalan shadow fixtures --lab-dir my-lab --max 20
```

It writes one fixture per decision (highest risk first), with the four probabilities recorded as `replay` and the decision as the assertion:

```yaml
replay:
  risks.destructive:
    answer: 0.62
    confidence: 0.04
  # …
expect:
  - cel: decision.verdict == 'ask'
    why: The recorded verdict (ask) must be reproduced
```

(Example with a test log.) From now on, if you change a threshold or a rule, `jevalan lab offline` tells you which real decisions would change. These fixtures keep a `command` from the log: review what matters before pushing them to any repository.

## What you have learned

- A `replay` fixes the model's answer so only your rules are tested.
- The `why` documents the rule; a threshold's edge is pinned with a fixture.
- Real cases come in through `shadow fixtures`, and from there CI warns you if something changes.

Next: [Lab](/en/laboratorio/) to measure real models (`record` and `live`) and the [capability catalog](/en/catalogo/) to see what else you can pin.
