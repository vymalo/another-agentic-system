// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { createMockServer } from "../../../../mock/server";

/*
 * The two reads and the one write of the MCP servers (ADR 0024), against the mock orchestrator: the
 * deployment's list, live and never cached, and what a person who may not have it is told; and `PUT
 * /api/threads/{id}/tools`, which makes the whole set and says why when it is refused.
 */

// the API client keeps the `fetch` it finds when it is created, so the hooks are imported after it is replaced
let useToolServers: typeof import("./use-tool-servers").useToolServers;
let useThreadTools: typeof import("./use-thread-tools").useThreadTools;

const server = createMockServer({ stepMs: 5, keepaliveMs: 50 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
let failing: Record<string, number> = {};
let held: Promise<void> | null = null;
let cookie = "";
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
    const method = input instanceof Request ? input.method : (init?.method ?? "GET");
    seen.push(`${method} ${url.pathname}`);
    if (held) await held;
    const status = failing[url.pathname];
    if (status) {
      return new Response(
        JSON.stringify({ title: "Failure", status, detail: "the server says no" }),
        {
          status,
          headers: { "Content-Type": "application/problem+json" },
        },
      );
    }
    const carried = new RealRequest(abs(input) as RequestInfo, init);
    if (cookie) carried.headers.set("cookie", cookie);
    return realFetch(carried);
  }) as typeof fetch;
  ({ useToolServers } = await import("./use-tool-servers"));
  ({ useThreadTools } = await import("./use-thread-tools"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});
afterEach(async () => {
  cleanup();
  vi.restoreAllMocks();
  failing = {};
  held = null;
  cookie = "";
  seen.length = 0;
  await realFetch(`${base}/__mock/reset`, { method: "POST" });
});

const post = (path: string, body?: unknown) =>
  realFetch(base + path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });

const ids = (servers: { id: string }[]) => servers.map((s) => s.id);

describe("useToolServers", () => {
  it("reads the deployment's servers in its order", async () => {
    const { result } = renderHook(() => useToolServers(true));
    expect(result.current.loading).toBe(true);
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(ids(result.current.servers)).toEqual(["websearch", "github", "docs"]);
    expect(result.current.unavailable).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it("reads nothing while it is not enabled (a person whose roles hold no thread.write)", () => {
    renderHook(() => useToolServers(false));
    expect(seen).toEqual([]);
  });

  it("is live: a server the deployment adds is there on the next read, without a reload", async () => {
    await post("/__mock/tool-servers?session=live", [{ id: "one", name: "One" }]);
    cookie = "mock-registry=live";
    const { result } = renderHook(() => useToolServers(true));
    await waitFor(() => expect(ids(result.current.servers)).toEqual(["one"]));
    await post("/__mock/tool-servers?session=live", [
      { id: "one", name: "One" },
      { id: "two", name: "Two" },
    ]);
    act(() => result.current.reload());
    await waitFor(() => expect(ids(result.current.servers)).toEqual(["one", "two"]));
  });

  it("is unavailable, and says nothing, for a role without thread.write (403) and an orchestrator that has no route (404)", async () => {
    for (const status of [403, 404]) {
      failing = { "/api/tool-servers": status };
      const { result, unmount } = renderHook(() => useToolServers(true));
      await waitFor(() => expect(result.current.unavailable).toBe(true));
      expect(result.current.servers).toEqual([]);
      expect(result.current.error).toBeNull();
      unmount();
    }
  });

  it("keeps the list on screen when a read fails, and says why", async () => {
    const { result } = renderHook(() => useToolServers(true));
    await waitFor(() => expect(result.current.servers).toHaveLength(3));
    failing = { "/api/tool-servers": 503 };
    act(() => result.current.reload());
    await waitFor(() => expect(result.current.error).toBe("the server says no"));
    expect(ids(result.current.servers)).toEqual(["websearch", "github", "docs"]);
    expect(result.current.unavailable).toBe(false);
    failing = {};
    act(() => result.current.reload());
    await waitFor(() => expect(result.current.error).toBeNull());
  });
});

describe("useThreadTools", () => {
  /** A finished thread of the default session, and its id. */
  async function thread(): Promise<string> {
    const id = "00000000-0000-4000-8000-0000000000aa";
    const res = await realFetch(`${base}/agui/agents/coder`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
      body: JSON.stringify({
        threadId: id,
        runId: "run-1",
        state: {},
        tools: [],
        context: [],
        forwardedProps: {},
        messages: [{ id: "m-1", role: "user", content: "echo tools" }],
      }),
    });
    await res.text();
    return id;
  }
  const args = (threadId: string | null, over: Record<string, unknown> = {}) => ({
    threadId,
    stream: undefined,
    streamSeq: 0,
    fetched: undefined,
    fetchedSeq: 5,
    refetchSoon: vi.fn(),
    ...over,
  });

  it("makes the whole set with one PUT and shows it at once, sorted", async () => {
    const id = await thread();
    const a = args(id);
    const { result } = renderHook(() => useThreadTools(a));
    expect(result.current.tools).toEqual([]);
    let taken = false;
    await act(async () => {
      taken = await result.current.set(["websearch", "docs", "docs"]);
    });
    expect(taken).toBe(true);
    expect(result.current.tools).toEqual(["docs", "websearch"]);
    expect(result.current.error).toBeNull();
    expect(a.refetchSoon).toHaveBeenCalledTimes(1);
    const log = (await (await realFetch(`${base}/api/threads/${id}`)).json()) as {
      tools: string[];
    };
    expect(log.tools).toEqual(["docs", "websearch"]);
    expect(seen.filter((c) => c.startsWith("PUT"))).toEqual([`PUT /api/threads/${id}/tools`]);
  });

  it("sends nothing for the set the thread has already", async () => {
    const id = await thread();
    const { result } = renderHook(() => useThreadTools(args(id, { fetched: ["docs"] })));
    await act(async () => {
      expect(await result.current.set(["docs"])).toBe(true);
    });
    expect(seen.filter((c) => c.startsWith("PUT"))).toEqual([]);
  });

  it("says why a change was refused, in the orchestrator's words, and keeps the set it had", async () => {
    const id = await thread();
    const { result } = renderHook(() => useThreadTools(args(id)));
    await act(async () => {
      expect(await result.current.set(["nope"])).toBe(false);
    });
    expect(result.current.error).toContain("nope");
    expect(result.current.tools).toEqual([]);
    // the next change that goes through clears it
    await act(async () => {
      expect(await result.current.set(["websearch"])).toBe(true);
    });
    expect(result.current.error).toBeNull();
  });

  it("refuses more than sixteen before asking", async () => {
    const id = await thread();
    const { result } = renderHook(() => useThreadTools(args(id)));
    await act(async () => {
      expect(await result.current.set(Array.from({ length: 17 }, (_, i) => `s${i}`))).toBe(false);
    });
    expect(result.current.error).toBe("At most 16 servers can be attached to a chat.");
    expect(seen.filter((c) => c.startsWith("PUT"))).toEqual([]);
  });

  it("is busy while the change is on its way", async () => {
    const id = await thread();
    const { result } = renderHook(() => useThreadTools(args(id)));
    let release: () => void = () => {};
    held = new Promise<void>((r) => {
      release = r;
    });
    let done: Promise<boolean> = Promise.resolve(false);
    act(() => {
      done = result.current.set(["websearch"]);
    });
    await waitFor(() => expect(result.current.busy).toBe(true));
    held = null;
    release();
    await act(async () => {
      await done;
    });
    expect(result.current.busy).toBe(false);
  });

  it("follows the log: the stream's snapshot, or the resource, whichever has read further", async () => {
    const { result, rerender } = renderHook((p) => useThreadTools(p), {
      initialProps: args("t", { stream: ["a"], streamSeq: 7, fetched: ["b"], fetchedSeq: 6 }),
    });
    expect(result.current.tools).toEqual(["a"]);
    rerender(args("t", { stream: ["a"], streamSeq: 7, fetched: ["b", "c"], fetchedSeq: 9 }));
    expect(result.current.tools).toEqual(["b", "c"]);
  });

  it("has no thread to change on the new-chat page", async () => {
    const { result } = renderHook(() => useThreadTools(args(null)));
    await act(async () => {
      expect(await result.current.set(["websearch"])).toBe(false);
    });
    expect(seen.filter((c) => c.startsWith("PUT"))).toEqual([]);
  });
});
