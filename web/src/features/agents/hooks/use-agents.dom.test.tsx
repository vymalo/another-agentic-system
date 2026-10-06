// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { createMockServer } from "../../../../mock/server";

/*
 * The agent list as the registry shapes it (ADR 0022), read against the mock orchestrator: the list
 * and the registry's state are read together, again on demand and when the window gets the focus
 * back, and a registry that cannot be read leaves the configured agents with a flag the picker
 * turns into words.
 */

// the API client keeps the `fetch` it finds when it is created, so the hook is imported after it is replaced
let useAgents: typeof import("./use-agents").useAgents;

const server = createMockServer({ stepMs: 5, keepaliveMs: 50 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** Paths the next fetches should fail with this status instead of reaching the mock. */
let failing: Record<string, number> = {};
/** How long the mock's answers are held, to see a hook in between. */
let held: Promise<void> | null = null;
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
    if (held) await held;
    const status = failing[url.pathname];
    if (status)
      return new Response("{}", { status, headers: { "Content-Type": "application/json" } });
    return realFetch(input instanceof Request ? input : (abs(input) as RequestInfo), init);
  }) as typeof fetch;
  ({ useAgents } = await import("./use-agents"));
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
  seen.length = 0;
  await realFetch(`${base}/__mock/reset`, { method: "POST" });
});

const post = (path: string, body?: unknown) =>
  realFetch(base + path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });

const ids = (agents: { id: string }[]) => agents.map((a) => a.id);

describe("useAgents", () => {
  it("reads the agents and the registry's state together", async () => {
    const { result } = renderHook(() => useAgents(true));
    expect(result.current.loading).toBe(true);
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(ids(result.current.agents)).toEqual(["adam", "reviewer", "verifier"]);
    expect(result.current.registry).toEqual({ unreachable: [] });
    expect(result.current.error).toBeNull();
    expect(seen.sort()).toEqual(["/api/agents", "/api/registry"]);
  });

  it("reads nothing while it is not enabled", () => {
    renderHook(() => useAgents(false));
    expect(seen).toEqual([]);
  });

  it("shows an agent the platform adds on the next read, without a reload", async () => {
    const { result } = renderHook(() => useAgents(true));
    await waitFor(() => expect(result.current.loading).toBe(false));
    await post("/__mock/registry/agents", {
      id: "helper",
      name: "Helper",
      tags: ["writing"],
    });
    act(() => result.current.retry());
    await waitFor(() => expect(ids(result.current.agents)).toContain("helper"));
    const helper = result.current.agents.find((a) => a.id === "helper");
    expect(helper).toMatchObject({ source: "registry", tags: ["writing"] });
    // the configured agents come first, so the default does not move
    expect(result.current.agents[0]?.id).toBe("adam");
  });

  it("flags a registry that cannot be read, keeps the configured agents, and notices when it is back", async () => {
    await post("/__mock/registry/agents", { id: "helper", name: "Helper" });
    const { result } = renderHook(() => useAgents(true));
    await waitFor(() => expect(ids(result.current.agents)).toContain("helper"));

    await post("/__mock/registry?down=true");
    act(() => result.current.retry());
    await waitFor(() => expect(result.current.registry.unreachable).toEqual(["platform"]));
    expect(ids(result.current.agents)).toEqual(["adam", "reviewer", "verifier"]);
    expect(result.current.error).toBeNull();

    await post("/__mock/registry?down=false");
    act(() => result.current.retry());
    await waitFor(() => expect(result.current.registry.unreachable).toEqual([]));
    expect(ids(result.current.agents)).toContain("helper");
  });

  it("does not lose the list when the registry's state cannot be read (an older orchestrator)", async () => {
    failing = { "/api/registry": 404 };
    const { result } = renderHook(() => useAgents(true));
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(ids(result.current.agents)).toHaveLength(3);
    expect(result.current.registry).toEqual({ unreachable: [] });
    expect(result.current.error).toBeNull();
  });

  it("keeps the list on screen while a refresh runs, and when it fails", async () => {
    const { result } = renderHook(() => useAgents(true));
    await waitFor(() => expect(result.current.loading).toBe(false));

    let release: () => void = () => {};
    held = new Promise<void>((r) => {
      release = r;
    });
    act(() => result.current.retry());
    // in between: the list is there, and nothing says it is loading
    expect(ids(result.current.agents)).toHaveLength(3);
    expect(result.current.loading).toBe(false);
    release();
    held = null;

    failing = { "/api/agents": 500 };
    act(() => result.current.retry());
    await waitFor(() => expect(result.current.error).not.toBeNull());
    expect(ids(result.current.agents)).toHaveLength(3);
  });

  it("reads again when the window gets the focus back, but not more than every five seconds", async () => {
    const { result } = renderHook(() => useAgents(true));
    await waitFor(() => expect(result.current.loading).toBe(false));
    const reads = () => seen.filter((p) => p === "/api/agents").length;
    expect(reads()).toBe(1);

    // right away: the list was just read
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(reads()).toBe(1);

    // later: it is read again, and a registry agent that appeared meanwhile shows
    await post("/__mock/registry/agents", { id: "helper", name: "Helper" });
    const now = Date.now();
    vi.spyOn(Date, "now").mockReturnValue(now + 6_000);
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(ids(result.current.agents)).toContain("helper"));
    expect(reads()).toBe(2);
  });
});
