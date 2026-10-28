import { parseEvent, type JevEvent } from "./events";

export interface DecisionResult {
  trace_id: string;
  verdict: string;
  confidence: number;
  stops_at?: string;
  route?: string;
}

export class JevClient {
  constructor(private baseUrl: string) {}

  async decide(input: string, client = "sdk-ts"): Promise<DecisionResult> {
    const res = await fetch(`${this.baseUrl}/v1/decide`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ input, client }),
    });
    if (!res.ok) {
      throw new Error(`decide failed: ${res.status}`);
    }
    return (await res.json()) as DecisionResult;
  }

  onEvent(cb: (e: JevEvent) => void): () => void {
    const wsUrl = this.baseUrl.replace(/^http/, "ws") + "/events";
    const ws = new WebSocket(wsUrl);
    ws.onmessage = (m) => {
      try {
        cb(parseEvent(typeof m.data === "string" ? m.data : ""));
      } catch {
        /* ignore malformed frames */
      }
    };
    return () => ws.close();
  }
}
