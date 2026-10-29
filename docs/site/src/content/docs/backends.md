---
title: Backends compatibles
description: Qué tiene que cumplir un servidor para servir de backend a Jevalan, cómo comprobarlo con tres peticiones y qué pasa cuando no lo cumple.
---

Cualquier servicio que hable el contrato de [System One](/system-one/) puede ser un backend de Jevalan: uno alojado, un servidor local sobre un modelo abierto o un simulador. Jevalan no distingue entre ellos. Esta página dice qué se le exige a uno, cómo comprobarlo antes de enchufarlo y qué ocurre cuando falla.

## Registrar un backend

Un backend es una entrada de `backends` en la [configuración](/configuracion/):

```yaml
backends:
  local: { url: "http://127.0.0.1:9000", timeout_ms: 5000 }
default_backend: local
```

Los flujos lo nombran con `backend: local`; el resto de la configuración (clave, ruta) está en [Configuración](/configuracion/).

## Qué se le exige

Un [flujo](/flujos/) lee la respuesta con reglas estrictas. Cualquier incumplimiento sale por el puerto `error` del nodo, no por un fallo de la petición:

| Requisito | Si no se cumple |
|---|---|
| Existe `answers.<pregunta>` con el mismo nombre que se envió. | `la respuesta no trae la pregunta <nombre>`. |
| Si la respuesta trae `type`, coincide con el del nodo. | `respuesta de tipo <x>, se esperaba <y>`. |
| Trae el campo que lleva el nombre del tipo: `noul`, `choice` o `score`. | `falta el campo <tipo>`. |
| `choice` y `score` traen `confidence`. En `noul` es opcional: si falta, Jevalan la calcula a partir de la probabilidad. | `falta confidence`. |
| `confidence` está entre 0 y 1. | `confianza fuera de [0, 1]`. |
| El valor de `noul` es un número. | `noul debe ser un número`. |

Además, el servidor debe aceptar **varias preguntas en una sola petición** si vas a usar nodos `batch` (un `batch` hace una llamada con todas), y los **tres tipos** (`noul`, `choice`, `score`) si tus flujos los usan. Un servidor que solo implementa algunos tipos no es defectuoso, pero cada nodo de un tipo no admitido saldrá por `error`: mira el ejemplo 4 de [Ejemplos](/ejemplos/).

## Comprobarlo con tres peticiones

Lánzalas **contra el servidor**, sin pasar por Jevalan, para saber de quién es el problema. Cambia la URL y `<model-id>`:

```sh
URL=http://127.0.0.1:9000/v1/systemone

# noul: devuelve una probabilidad de sí
curl -s $URL -H 'Content-Type: application/json' -d '{"model":"<model-id>","state":"rm -rf /","questions":{"q":{"type":"noul","instructions":"Is the command destructive?"}}}'

# choice: devuelve una de las claves, con probabilities y confidence
curl -s $URL -H 'Content-Type: application/json' -d '{"model":"<model-id>","state":"The app crashes on export.","questions":{"q":{"type":"choice","instructions":"What does the user want?","criteria":{"bug":"Something is broken","howto":"A usage question","other":"Anything else"}}}}'

# score: devuelve una posición de la escala, con confidence
curl -s $URL -H 'Content-Type: application/json' -d '{"model":"<model-id>","state":"The app crashes on export.","questions":{"q":{"type":"score","instructions":"How urgent is it?","criteria":["none","low","medium","high"]}}}'
```

Cada respuesta debe parecerse a las de [la referencia](/system-one/): un `answers.q` con su `type` y el campo del tipo. Para el caso de varias preguntas, repite la primera con dos preguntas con nombres distintos y comprueba que vuelven las dos.

Cuando las tres funcionan, repítelas contra Jevalan (`POST /local/v1/systemone`, mira [API HTTP](/api/)) para comprobar la clave y el plazo, y ejecuta un flujo de un solo nodo para ver su `path`. Si el nodo sale por `error`, el motivo está en el [rastro](/solucion-de-problemas/).

## Cuando algo no encaja

| Síntoma | Causa probable |
|---|---|
| Todos los nodos de un tipo salen por `error`. | El servidor no implementa ese tipo. |
| `falta confidence` en `choice` o `score`. | El servidor devuelve probabilidades pero no el campo `confidence`. |
| El `batch` falla y los nodos sueltos no. | El servidor solo admite una pregunta por petición. |
| Salen `502` o tiempos agotados. | `url`, `path` o `timeout_ms` mal puestos, o el servidor es lento con tu `state`. |
| Las decisiones cambian entre ejecuciones. | Normal en un modelo: mídelo con repeticiones en el [laboratorio](/laboratorio/). |

Un servidor distinto no obtiene los mismos números. Los umbrales de un flujo se ajustan a un backend: cuando cambies de backend, vuelve a medirlos con el laboratorio (`jevalan lab live`) antes de pasar a `enforce`.
