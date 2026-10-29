import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { parseEvent } from "./events";

const here = dirname(fileURLToPath(import.meta.url));
const fixture = resolve(here, "../../../config/schema/fixtures/decision_made.json");

describe("event contract", () => {
  it("parses the shared Go fixture", () => {
    const raw = readFileSync(fixture, "utf8");
    const e = parseEvent(raw);
    expect(e.v).toBe(1);
    expect(e.type).toBe("DECISION_MADE");
    expect(e.trace_id).toBe("01HZ1111111111111111111111");
    if (e.type === "DECISION_MADE") {
      expect(e.payload.confidence).toBeCloseTo(0.85);
      expect(e.payload.route).toBe("code-edit");
    }
  });

  it("rejects unknown event types", () => {
    expect(() => parseEvent(JSON.stringify({ v: 1, type: "NOPE" }))).toThrow();
  });
});
