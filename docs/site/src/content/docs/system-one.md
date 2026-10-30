---
title: Referencia System One
description: El contrato de System One, la API de decisiones tipadas que Jevalan reenvía y que sus flujos usan, con un OpenAPI descargable.
---

System One es una API de decisiones rápidas y tipadas: envías un `state` y hasta 64 preguntas con nombre, y devuelve una respuesta tipada, con confianza, por cada una. Jevalan la reenvía tal cual (passthrough) y es lo que hablan por debajo los nodos `noul`, `choice` y `score` de un [flujo](/flujos/).

:::caution[No es la especificación oficial]
Esta página describe el contrato **tal como lo observa Jevalan**: su código, su servidor de pruebas y la documentación pública de pasarelas que exponen el mismo endpoint. Lo que esas fuentes no fijan (el cuerpo de los errores, los límites de tamaño) no se inventa aquí. Ante una duda, manda la documentación de tu proveedor.
:::

Descarga la descripción OpenAPI 3.1 de este contrato: [`system-one.openapi.yaml`](/system-one.openapi.yaml). Sirve para generar un cliente, validar peticiones o abrirla en cualquier visor de OpenAPI.

## Endpoint

`POST /v1/systemone`, cuerpo y respuesta en JSON, con `Authorization: Bearer <clave>`. A través de Jevalan:

| Ruta | A quién llega |
|---|---|
| `POST /v1/systemone` | El backend predeterminado (`default_backend`). |
| `POST /{backend}/v1/systemone` | El backend que nombras. |

Jevalan reenvía el cuerpo byte a byte, añade la clave del backend si hay una configurada y devuelve el estado y el cuerpo del backend sin tocar. Si el backend no responde, contesta `502`. Detalles en [API HTTP](/api/) y [Configuración](/configuracion/).

## La petición

| Campo | Tipo | Significado |
|---|---|---|
| `model` | texto | Identificador del modelo en ese backend. |
| `state` | texto | Lo que se juzga. |
| `questions` | objeto | Preguntas con nombre: de 1 a 64. |

Cada pregunta lleva un `type` y, según el tipo:

| `type` | `instructions` | `criteria` | Responde |
|---|---|---|---|
| `noul` | La pregunta, formulada para sí o no. | Opcional: solo las claves `"true"` y `"false"`, con lo que significa cada una. | Una probabilidad de sí. |
| `choice` | La pregunta. | Obligatorio: mapa clave → descripción de cada opción. | Una de las claves. |
| `score` | La pregunta. | Obligatorio: lista de niveles, del más bajo al más alto. | Una posición en esa escala. |

```json
{
  "model": "<model-id>",
  "state": "I was charged twice for my subscription.",
  "questions": {
    "refund": { "type": "noul", "instructions": "Is the customer asking for money back?" },
    "department": {
      "type": "choice",
      "instructions": "Which team should handle this?",
      "criteria": { "billing": "Charges and refunds", "technical": "Bugs and outages" }
    },
    "urgency": { "type": "score", "instructions": "How urgent is it?", "criteria": ["none", "low", "medium", "high"] }
  }
}
```

## La respuesta

Una entrada en `answers` por pregunta, con el mismo nombre, más el `usage` de la llamada:

| Tipo | Campos de la respuesta |
|---|---|
| `noul` | `noul` (probabilidad de sí, de 0 a 1) y `confidence`. |
| `choice` | `choice` (la clave elegida), `probabilities` (una por clave) y `confidence`. |
| `score` | `score` (posición en la escala, de 0 al índice del último nivel) y `confidence`. |

```json
{
  "model": "<model-id>",
  "answers": {
    "refund": { "type": "noul", "noul": 0.98, "confidence": 0.9 },
    "department": { "type": "choice", "choice": "billing", "probabilities": { "billing": 0.91, "technical": 0.09 }, "confidence": 0.88 },
    "urgency": { "type": "score", "score": 2.1, "confidence": 0.74 }
  },
  "usage": { "input_tokens": 275, "output_tokens": 20 }
}
```

Los números del ejemplo son de muestra. La respuesta no se emite por trozos: llega entera.

## Cómo lo usa un flujo

Un nodo de [flujo](/flujos/) es una pregunta System One con puertos de salida:

| Nodo | Pregunta | Puertos de salida |
|---|---|---|
| `noul` | `type: noul` | `"true"` / `"false"` según el `threshold`. |
| `choice` | `type: choice`, con `criteria` | Uno por clave de `criteria`. |
| `score` | `type: score`, con `criteria` | Rangos `a..b` sobre la escala. |
| `batch` | Varias preguntas en una sola llamada | Las reglas `route` ven todas las respuestas. |

Además, cualquier nodo tiene los puertos `error` (el backend falló o contestó mal) y `low-confidence` (la `confidence` quedó por debajo de `min_confidence`). Por eso un backend que no admite un tipo de pregunta hace salir cada petición por `error`: mira el ejemplo 4 de [Ejemplos](/ejemplos/).

## Errores

Las fuentes públicas solo fijan algunos casos: una pregunta `noul` sin `instructions` ni `criteria` responde `400`, una clave incorrecta `401`, y un acceso denegado por una pasarela `403`. No hay un esquema común para el cuerpo del error, así que trata cualquier estado que no sea `200` como «sin respuesta» y decide el valor por defecto como explica la [guía de decisiones](/guia-decisiones/).
