// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { THREAD_TOOLS_URI } from "@/features/tools/lib/servers";
import { createMockServer } from "../../../../mock/server";

/*
 * What the agent's card lists (`GET /agui/agents/{id}/capabilities`, ADR 0008), read live: the key of
 * an extension in `custom` is the signal, a card that cannot be read says nothing, and an answer
 * about one agent is never taken for another's.
 */

let useAgentCapabilities: typeof import("./use-agent-capabilities").useAgentCapabilities;

const server = createMockServer({ stepMs: 5, keepaliveMs: 50 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
let failing: Record<string, number> = {};
const seen: string[] = [];

beforeAll(async () => {
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const abs = (input: unknown) =>
    typeof input === "string" && input.startsWith("/") ? base + input : input;
  globalThis.Request = class extends RealRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(abs(input) as RequestInfo, init);
    }
  } as typeof Request;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(input instanceof Request ? input.url : String(abs(input)));
    seen.push(url.pathname);
    const status = failing[url.pathname];
    if (status)
      return new Response("{}", { status, headers: { "Content-Type": "application/json" } });
    return realFetch(input instanceof Request ? input : (abs(input) as RequestInfo), init);
  }) as typeof fetch;
  ({ useAgentCapabilities } = await import("./use-agent-capabilities"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});
afterEach(() => {
  cleanup();
  failing = {};
  seen.length = 0;
});

describe("useAgentCapabilities", () => {
  it("says an agent whose card lists thread-tools/v1 supports it, and one that does not, not", async () => {
    const coder = renderHook(() => useAgentCapabilities("coder"));
    expect(coder.result.current.status).toBe("unknown");
    expect(coder.result.current.supports(THREAD_TOOLS_URI)).toBeNull();
    await waitFor(() => expect(coder.result.current.status).toBe("ready"));
    expect(coder.result.current.supports(THREAD_TOOLS_URI)).toBe(true);
    expect(coder.result.current.supports("https://example.org/other/v1")).toBe(false);

    const reviewer = renderHook(() => useAgentCapabilities("reviewer"));
    await waitFor(() => expect(reviewer.result.current.status).toBe("ready"));
    expect(reviewer.result.current.supports(THREAD_TOOLS_URI)).toBe(false);
  });

  it("reads nothing with no agent chosen", () => {
    const { result } = renderHook(() => useAgentCapabilities(null));
    expect(result.current.status).toBe("unknown");
    expect(seen).toEqual([]);
  });

  it("is live: it reads the card again on refresh, and again for another agent, never taking one for the other", async () => {
    const { result, rerender } = renderHook(({ id }) => useAgentCapabilities(id), {
      initialProps: { id: "coder" as string | null },
    });
    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(result.current.supports(THREAD_TOOLS_URI)).toBe(true);
    const before = seen.length;
    act(() => result.current.refresh());
    await waitFor(() => expect(seen.length).toBeGreaterThan(before));

    rerender({ id: "reviewer" });
    // in between, the coder's answer is not the reviewer's
    expect(result.current.supports(THREAD_TOOLS_URI)).not.toBe(true);
    await waitFor(() => expect(result.current.supports(THREAD_TOOLS_URI)).toBe(false));
  });

  it("is unreadable, and supports nothing, when the document cannot be read", async () => {
    failing = { "/agui/agents/coder/capabilities": 503 };
    const { result } = renderHook(() => useAgentCapabilities("coder"));
    await waitFor(() => expect(result.current.status).toBe("unreadable"));
    expect(result.current.supports(THREAD_TOOLS_URI)).toBeNull();
  });
});
