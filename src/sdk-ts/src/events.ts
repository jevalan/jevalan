export interface EventEnvelope {
  v: number;
  id: string;
  ts: string;
  trace_id: string;
  seq: number;
  type: string;
  source: string;
  payload?: unknown;
}

export interface JevEventBase {
  v: number;
  id: string;
  ts: string;
  trace_id: string;
  seq: number;
  source: string;
}

export type JevEvent =
  | (JevEventBase & { type: "REQUEST_RECEIVED"; payload: { endpoint: string; client: string } })
  | (JevEventBase & { type: "HOOK_START"; payload: { hook: string; input_digest: string } })
  | (JevEventBase & { type: "ROUTED"; payload: { target: string; reason: string } })
  | (JevEventBase & { type: "CONFIDENCE_SCORE"; payload: { hook: string; score: number } })
  | (JevEventBase & { type: "HOOK_END"; payload: { hook: string; verdict: string; duration_ms: number } })
  | (JevEventBase & { type: "DECISION_MADE"; payload: { verdict: string; confidence: number; stops_at?: string; route?: string } })
  | (JevEventBase & { type: "ERROR"; payload: { stage: string; message: string } })
  | (JevEventBase & { type: "LAGGED"; payload: { dropped: number } });

const KNOWN_TYPES = new Set([
  "REQUEST_RECEIVED",
  "HOOK_START",
  "ROUTED",
  "CONFIDENCE_SCORE",
  "HOOK_END",
  "DECISION_MADE",
  "ERROR",
  "LAGGED",
]);

export function parseEvent(raw: string): JevEvent {
  const env = JSON.parse(raw) as EventEnvelope;
  if (typeof env.v !== "number" || !KNOWN_TYPES.has(env.type)) {
    throw new Error(`unknown or malformed event: ${env.type}`);
  }
  return env as unknown as JevEvent;
}
