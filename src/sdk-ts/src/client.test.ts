import { afterEach, describe, expect, it, vi } from "vitest";
import { JevClient } from "./client";

describe("JevClient.decide", () => {
  afterEach(() => vi.restoreAllMocks());

  it("posts the prompt and returns the decision", async () => {
    const fetchMock = vi.fn(async () => ({
      ok: true,
      json: async () => ({ trace_id: "t1", verdict: "allow", confidence: 0.7, route: "code-edit" }),
    }));
    vi.stubGlobal("fetch", fetchMock);

    const result = await new JevClient("http://localhost:8080").decide("refactor this");
    expect(result.route).toBe("code-edit");

    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("http://localhost:8080/v1/decide");
    expect(JSON.parse(init.body as string).input).toBe("refactor this");
  });

  it("throws on a non-ok response", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 500 })));
    await expect(new JevClient("http://localhost:8080").decide("x")).rejects.toThrow();
  });
});
