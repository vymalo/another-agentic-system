// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { uuidv7 } from "@/lib/uuid";
import { createMockServer } from "../../../../mock/server";

/*
 * The person's own list of threads (ADR 0042), read against the mock orchestrator: `order=rail`
 * (the server's order, blocks of a thread and the threads nested under it), the archived ones read
 * only when the section is open, and every arrangement one `arrangeThread` call followed by a
 * re-read, because the ranks are the server's.
 */

// the API client keeps the `fetch` it finds when it is created, so the hook is imported after it is replaced
let useThreads: typeof import("./use-threads").useThreads;

const server = createMockServer({ stepMs: 5, keepaliveMs: 50 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** The mock session the hook's requests carry (the cookie its browser would), so the list is the test's own. */
let cookie = "";
/** Every call the hook made: `METHOD path?query status`. */
let calls: string[] = [];
/** The next request to this `METHOD path` fails with this problem instead of reaching the mock. */
let failing: { key: string; status: number; detail: string; code?: string } | undefined;

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
    const request = input instanceof Request ? input : undefined;
    const url = new URL(request ? request.url : String(abs(String(input))));
    const method = request?.method ?? init?.method ?? "GET";
    const key = `${method} ${url.pathname}`;
    if (failing && key === failing.key) {
      calls.push(`${key}${url.search} ${failing.status}`);
      const body = JSON.stringify({
        title: "Failure",
        status: failing.status,
        detail: failing.detail,
        ...(failing.code ? { code: failing.code } : {}),
      });
      failing = undefined;
      return new Response(body, {
        status: JSON.parse(body).status,
        headers: { "content-type": "application/problem+json" },
      });
    }
    const carried = new RealRequest(abs(input) as RequestInfo, init);
    if (cookie) carried.headers.set("cookie", cookie);
    const res = await realFetch(carried);
    calls.push(`${key}${url.search} ${res.status}`);
    return res;
  }) as typeof fetch;
  ({ useThreads } = await import("./use-threads"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});

let people = 0;
/** Makes the hook's requests those of a person of its own, so the list is theirs alone. */
async function aPerson() {
  const session = `hook-${++people}`;
  const user = `hook${people}@example.com`;
  await realFetch(`${base}/__mock/config?user=${user}&session=${session}`, { method: "POST" });
  cookie = `mock-registry=${session}`;
}

afterEach(async () => {
  cleanup();
  vi.restoreAllMocks();
  calls = [];
  failing = undefined;
  cookie = "";
  await realFetch(`${base}/__mock/reset`, { method: "POST" });
});

/** A thread of the current person, run to its end. */
async function threadOf(text: string): Promise<string> {
  const threadId = uuidv7();
  const res = await realFetch(`${base}/agui/agents/adam`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream", cookie },
    body: JSON.stringify({
      threadId,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: text }],
    }),
  });
  expect(res.status).toBe(200);
  await res.text();
  return threadId;
}

/** "Fork from here" on the first turn of a thread of the current person. */
async function forkOf(parent: string): Promise<string> {
  const exported = await realFetch(`${base}/api/threads/${parent}/export`, { headers: { cookie } });
  const log = (await exported.json()) as { events: { seq: number; kind: string }[] };
  const after = log.events.find((e) => e.kind === "user_message")?.seq ?? 1;
  const res = await realFetch(`${base}/api/threads/${parent}/fork`, {
    method: "POST",
    headers: { "Content-Type": "application/json", cookie },
    body: JSON.stringify({ after }),
  });
  expect(res.status).toBe(201);
  return ((await res.json()) as { id: string }).id;
}

const ids = (rows: { id: string }[]) => rows.map((r) => r.id);
const loaded = async (result: { current: { loading: boolean } }) =>
  waitFor(() => expect(result.current.loading).toBe(false));

describe("useThreads", () => {
  it("reads the person's list in their own order, with each fork under its parent", async () => {
    await aPerson();
    const a = await threadOf("echo a");
    const b = await threadOf("echo b");
    const kid = await forkOf(a);
    const { result } = renderHook(() => useThreads(""));
    await loaded(result);
    expect(ids(result.current.threads)).toEqual([b, a, kid]);
    expect(result.current.threads.find((t) => t.id === kid)?.nestedUnder).toBe(a);
    expect(calls.some((c) => c.startsWith("GET /api/threads?") && c.includes("order=rail"))).toBe(
      true,
    );
    // the archived are not read until the section is opened
    expect(calls.some((c) => c.includes("archived=only"))).toBe(false);
  });

  it("pins with one arrangeThread call and reads the list again: the thread is on top", async () => {
    await aPerson();
    const a = await threadOf("echo a");
    const b = await threadOf("echo b");
    const { result } = renderHook(() => useThreads(""));
    await loaded(result);
    expect(ids(result.current.threads)).toEqual([b, a]);
    const pinning = result.current.threads.find((t) => t.id === a);
    if (!pinning) throw new Error("no row");
    await act(async () => {
      await result.current.arrange(pinning, "pin");
    });
    await waitFor(() => expect(ids(result.current.threads)).toEqual([a, b]));
    expect(result.current.threads[0]?.pinned).toBe(true);
    expect(calls.filter((c) => c.startsWith(`PATCH /api/threads/${a}/rail`))).toHaveLength(1);
  });

  it("archives a thread out of the list and reads the archived only once the section is open", async () => {
    await aPerson();
    const a = await threadOf("echo a");
    const b = await threadOf("echo b");
    const { result } = renderHook(() => useThreads(""));
    await loaded(result);
    const archiving = result.current.threads.find((t) => t.id === a);
    if (!archiving) throw new Error("no row");
    await act(async () => {
      await result.current.arrange(archiving, "archive");
    });
    await waitFor(() => expect(ids(result.current.threads)).toEqual([b]));
    expect(result.current.archived.threads).toEqual([]);
    expect(calls.some((c) => c.includes("archived=only"))).toBe(false);

    act(() => result.current.archived.toggle());
    expect(result.current.archived.open).toBe(true);
    await waitFor(() => expect(ids(result.current.archived.threads)).toEqual([a]));
    expect(calls.some((c) => c.includes("archived=only") && c.includes("order=rail"))).toBe(true);

    // unarchive from the open section: it goes back to Chats where it was, and leaves Archived
    const restoring = result.current.archived.threads[0];
    if (!restoring) throw new Error("no archived row");
    await act(async () => {
      await result.current.arrange(restoring, "unarchive");
    });
    await waitFor(() => expect(ids(result.current.threads)).toEqual([b, a]));
    await waitFor(() => expect(result.current.archived.threads).toEqual([]));
  });

  it("forgets the archived ones when the section is closed, and reads them again when it opens", async () => {
    await aPerson();
    const a = await threadOf("echo a");
    const { result } = renderHook(() => useThreads(""));
    await loaded(result);
    const archiving = result.current.threads.find((t) => t.id === a);
    if (!archiving) throw new Error("no row");
    await act(async () => {
      await result.current.arrange(archiving, "archive");
    });
    act(() => result.current.archived.toggle());
    await waitFor(() => expect(ids(result.current.archived.threads)).toEqual([a]));
    act(() => result.current.archived.toggle());
    expect(result.current.archived.open).toBe(false);
    const reads = calls.filter((c) => c.includes("archived=only")).length;
    act(() => result.current.archived.toggle());
    await waitFor(() =>
      expect(calls.filter((c) => c.includes("archived=only")).length).toBeGreaterThan(reads),
    );
    await waitFor(() => expect(ids(result.current.archived.threads)).toEqual([a]));
  });

  it("ejects a fork from its parent: it is a row of its own, right after the block", async () => {
    await aPerson();
    const a = await threadOf("echo a");
    const kid = await forkOf(a);
    const { result } = renderHook(() => useThreads(""));
    await loaded(result);
    const ejecting = result.current.threads.find((t) => t.id === kid);
    if (!ejecting) throw new Error("no row");
    await act(async () => {
      await result.current.arrange(ejecting, "eject");
    });
    await waitFor(() =>
      expect(result.current.threads.find((t) => t.id === kid)?.nestedUnder).toBeUndefined(),
    );
    expect(ids(result.current.threads)).toEqual([a, kid]);
  });

  it("reads the list again after a bad anchor or a nested row (422), with a line that says so", async () => {
    await aPerson();
    const a = await threadOf("echo a");
    await threadOf("echo b");
    const { result } = renderHook(() => useThreads(""));
    await loaded(result);
    const row = result.current.threads.find((t) => t.id === a);
    if (!row) throw new Error("no row");
    const before = calls.filter((c) => c.startsWith("GET /api/threads?")).length;
    failing = {
      key: `PATCH /api/threads/${a}/rail`,
      status: 422,
      detail: "a thread nested under another cannot be pinned",
      code: "nested_row",
    };
    await act(async () => {
      await result.current.arrange(row, "pin");
    });
    await waitFor(() =>
      expect(calls.filter((c) => c.startsWith("GET /api/threads?")).length).toBeGreaterThan(before),
    );
    expect(result.current.notice).toMatch(/list changed/i);
    act(() => result.current.dismissNotice());
    expect(result.current.notice).toBeNull();
  });

  it("says why in the server's words when an arrangement is refused (403), and keeps the list", async () => {
    await aPerson();
    const a = await threadOf("echo a");
    const { result } = renderHook(() => useThreads(""));
    await loaded(result);
    const row = result.current.threads.find((t) => t.id === a);
    if (!row) throw new Error("no row");
    failing = {
      key: `PATCH /api/threads/${a}/rail`,
      status: 403,
      detail: "your roles do not let you do that",
      code: "forbidden",
    };
    await act(async () => {
      await result.current.arrange(row, "archive");
    });
    expect(result.current.notice).toBe(
      "Could not archive “echo a”: your roles do not let you do that",
    );
    expect(ids(result.current.threads)).toEqual([a]);
  });

  it("drops a thread that was deleted, and the threads that were nested under it take its place", async () => {
    await aPerson();
    const a = await threadOf("echo a");
    const kid = await forkOf(a);
    const { result } = renderHook(() => useThreads(""));
    await loaded(result);
    expect(ids(result.current.threads)).toEqual([a, kid]);
    await realFetch(`${base}/api/threads/${a}`, { method: "DELETE", headers: { cookie } });
    act(() => result.current.forget(a));
    // gone at once, without waiting for the read
    expect(ids(result.current.threads)).not.toContain(a);
    await waitFor(() => expect(ids(result.current.threads)).toEqual([kid]));
    expect(result.current.threads[0]?.nestedUnder).toBeUndefined();
  });

  it("pages by the last top-level thread, never splitting a block", async () => {
    await aPerson();
    const a = await threadOf("echo a");
    const b = await threadOf("echo b");
    const c = await threadOf("echo c");
    const kid = await forkOf(b);
    const { result } = renderHook(() => useThreads("", { page: 2 }));
    await loaded(result);
    // two threads of the list's own level and the fork under the second
    expect(ids(result.current.threads)).toEqual([c, b, kid]);
    expect(result.current.hasMore).toBe(true);
    act(() => result.current.loadMore());
    await waitFor(() => expect(ids(result.current.threads)).toEqual([c, b, kid, a]));
    expect(result.current.hasMore).toBe(false);
    expect(calls.some((c2) => c2.includes(`before=${b}`))).toBe(true);
  });

  it("keeps the pages the person loaded when it reads the list again", async () => {
    await aPerson();
    const a = await threadOf("echo a");
    await threadOf("echo b");
    await threadOf("echo c");
    const { result } = renderHook(() => useThreads("", { page: 2 }));
    await loaded(result);
    act(() => result.current.loadMore());
    await waitFor(() => expect(result.current.threads).toHaveLength(3));
    act(() => result.current.refresh());
    await waitFor(() =>
      expect(calls.filter((c) => c.includes("before=")).length).toBeGreaterThan(1),
    );
    await waitFor(() => expect(ids(result.current.threads).at(-1)).toBe(a));
    expect(result.current.threads).toHaveLength(3);
  });

  it("says Could not load threads in words when the list cannot be read", async () => {
    await aPerson();
    failing = { key: "GET /api/threads", status: 503, detail: "storage is unavailable" };
    const { result } = renderHook(() => useThreads(""));
    await loaded(result);
    expect(result.current.error).toBe("storage is unavailable");
  });
});
