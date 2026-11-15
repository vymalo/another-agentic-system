// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";

/*
 * `/api/prompts` (ADR 0070) as the page uses it: the list is read page by page until the server has no more, a read that is
 * overtaken is dropped, every write carries what the person was looking at, a refusal is told by its code, a write whose outcome
 * is not known is followed by a read, and nothing is kept on the device.
 */
const client = vi.hoisted(() => ({
  GET: vi.fn(),
  POST: vi.fn(),
  PATCH: vi.fn(),
  DELETE: vi.fn(),
}));
vi.mock("@/lib/api/client", async () => ({
  ...(await vi.importActual<typeof import("@/lib/api/client")>("@/lib/api/client")),
  api: client,
}));

import { usePromptLibrary } from "./use-prompt-library";

const CONFIG = { limit: 100, maxVariables: 8, maxBodyBytes: 16384, maxRenderedBytes: 32768 };

const summary = (over: Partial<ApiPromptSummary> = {}): ApiPromptSummary => ({
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review",
  description: "",
  variableCount: 0,
  own: true,
  revision: "1",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

const full = (over: Partial<ApiPrompt> = {}): ApiPrompt => ({
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review",
  description: "",
  body: "Hello",
  variables: [],
  own: true,
  audience: [],
  revision: "2",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

const page = (items: ApiPromptSummary[], nextCursor?: string) => ({
  data: {
    limit: 100,
    held: items.filter((i) => i.own).length,
    items,
    ...(nextCursor ? { nextCursor } : {}),
  },
  response: { status: 200, ok: true, headers: new Headers() },
});

const refused = (status: number, error: object = { title: "x", status }) => ({
  error,
  response: { status, ok: false, headers: new Headers() },
});

const deferred = <T,>() => {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
};

beforeEach(() => {
  client.GET.mockReset().mockResolvedValue(page([summary()]));
  client.POST.mockReset();
  client.PATCH.mockReset();
  client.DELETE.mockReset();
});
afterEach(cleanup);

async function ready() {
  const hook = renderHook(() => usePromptLibrary(CONFIG));
  await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
  return hook;
}

const prompts = (hook: Awaited<ReturnType<typeof ready>>) => {
  const s = hook.result.current.state;
  if (s.status !== "ready") throw new Error("not ready");
  return s.prompts;
};

describe("reading the list", () => {
  it("pages until nextCursor is absent, sending each cursor back", async () => {
    client.GET.mockReset()
      .mockResolvedValueOnce(page([summary({ id: "p-1", name: "b" })], "p-1"))
      .mockResolvedValueOnce(page([summary({ id: "p-2", name: "a" })], "p-2"))
      .mockResolvedValueOnce(page([summary({ id: "p-3", name: "c", own: false })]));
    const hook = await ready();
    expect(client.GET).toHaveBeenCalledTimes(3);
    expect(client.GET.mock.calls.map((c) => c[1])).toEqual([
      { params: { query: {} } },
      { params: { query: { cursor: "p-1" } } },
      { params: { query: { cursor: "p-2" } } },
    ]);
    expect(prompts(hook).map((p) => p.id)).toEqual(["p-2", "p-1", "p-3"]);
  });

  it("does not follow a cursor that repeats", async () => {
    // a server that never stops naming the same cursor: after 20 reads the mock ends it, so a loop fails the count and does not hang
    client.GET.mockReset().mockImplementation(async () =>
      client.GET.mock.calls.length > 20
        ? page([summary({ id: "p-1" })])
        : page([summary({ id: "p-1" })], "p-1"),
    );
    const hook = await ready();
    expect(client.GET.mock.calls.length).toBe(2);
    expect(prompts(hook)).toHaveLength(1);
  });

  it("says a 404 is a deployment with no library, a 403 a person whose roles grant nothing, a 401 a signed-out person", async () => {
    for (const [status, expected] of [
      [404, "off"],
      [403, "no-access"],
      [401, "signed-out"],
    ] as const) {
      client.GET.mockReset().mockResolvedValueOnce(refused(status));
      const hook = renderHook(() => usePromptLibrary(CONFIG));
      await waitFor(() => expect(hook.result.current.state.status).toBe(expected));
      cleanup();
    }
  });

  it("a first read that fails says so and reads again on reload", async () => {
    client.GET.mockReset().mockResolvedValueOnce(refused(503));
    const hook = renderHook(() => usePromptLibrary(CONFIG));
    await waitFor(() => expect(hook.result.current.state.status).toBe("error"));
    client.GET.mockResolvedValue(page([summary()]));
    act(() => hook.result.current.reload());
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
  });

  it("a slower first read never replaces a later one", async () => {
    const slow = deferred<ReturnType<typeof page>>();
    client.GET.mockReset().mockReturnValueOnce(slow.promise);
    const hook = renderHook(() => usePromptLibrary(CONFIG));
    client.GET.mockResolvedValueOnce(page([summary({ id: "p-new", name: "new" })]));
    act(() => hook.result.current.reload());
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    await act(async () => {
      slow.resolve(page([summary({ id: "p-old", name: "old" })]));
      await slow.promise;
    });
    expect(prompts(hook as never).map((p) => p.id)).toEqual(["p-new"]);
  });

  it("a failed reread keeps the list and says why beside it", async () => {
    const hook = await ready();
    client.GET.mockResolvedValueOnce(
      refused(503, { title: "x", status: 503, detail: "Storage is unavailable" }),
    );
    act(() => hook.result.current.reload());
    await waitFor(() =>
      expect(hook.result.current.state).toMatchObject({
        status: "ready",
        readError: expect.any(String),
      }),
    );
    expect(prompts(hook)).toHaveLength(1);
    client.GET.mockResolvedValueOnce(page([summary(), summary({ id: "p-2", name: "z" })]));
    act(() => hook.result.current.reload());
    await waitFor(() => expect(prompts(hook)).toHaveLength(2));
    expect(hook.result.current.state).toMatchObject({ readError: null });
  });

  it("reads again when the window regains focus, and not twice within a quarter second", async () => {
    await ready();
    const before = client.GET.mock.calls.length;
    await new Promise((r) => setTimeout(r, 300));
    act(() => {
      window.dispatchEvent(new Event("focus"));
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(client.GET.mock.calls.length).toBe(before + 1));
  });
});

describe("writing", () => {
  it("create sends the body, puts the prompt in the list and answers it", async () => {
    const hook = await ready();
    client.POST.mockResolvedValueOnce({
      data: full({
        id: "p-bbbbbbbbbbbb",
        name: "zeta",
        variables: [{ id: "a", question: "A?", type: "text" }],
      }),
      response: { status: 201, ok: true, headers: new Headers() },
    });
    let result: Awaited<ReturnType<typeof hook.result.current.create>> | undefined;
    await act(async () => {
      result = await hook.result.current.create(
        { name: "zeta", title: "Z", body: "{{a}}" },
        { name: "zeta", body: "{{a}}", ids: ["a"] },
      );
    });
    expect(result?.ok).toBe(true);
    expect(client.POST).toHaveBeenCalledWith("/api/prompts", {
      body: { name: "zeta", title: "Z", body: "{{a}}" },
    });
    const added = prompts(hook).find((p) => p.id === "p-bbbbbbbbbbbb");
    expect(added).toMatchObject({ variableCount: 1, own: true, name: "zeta" });
    expect(added).not.toHaveProperty("body");
  });

  it("update sends the id in the path and the revision in the body, and the answer replaces the row", async () => {
    const hook = await ready();
    client.PATCH.mockResolvedValueOnce({
      data: full({ title: "Renamed", revision: "9" }),
      response: { status: 200, ok: true, headers: new Headers() },
    });
    await act(async () => {
      await hook.result.current.update(
        "p-aaaaaaaaaaaa",
        { revision: "1", title: "Renamed" },
        { name: "review", body: "Hello", ids: [] },
      );
    });
    expect(client.PATCH).toHaveBeenCalledWith("/api/prompts/{promptId}", {
      params: { path: { promptId: "p-aaaaaaaaaaaa" } },
      body: { revision: "1", title: "Renamed" },
    });
    expect(prompts(hook)[0]).toMatchObject({ title: "Renamed", revision: "9" });
  });

  it("stale and 503 reread, and say so in the words of their code", async () => {
    const hook = await ready();
    const ctx = { name: "review", body: "Hello", ids: [] };
    client.PATCH.mockResolvedValueOnce(
      refused(409, { title: "x", status: 409, code: "stale_revision" }),
    );
    const before = client.GET.mock.calls.length;
    let stale: Awaited<ReturnType<typeof hook.result.current.update>> | undefined;
    await act(async () => {
      stale = await hook.result.current.update(
        "p-aaaaaaaaaaaa",
        { revision: "1", title: "x" },
        ctx,
      );
    });
    expect(stale).toMatchObject({ ok: false, failure: { stale: true, reread: true } });
    await waitFor(() => expect(client.GET.mock.calls.length).toBe(before + 1));

    client.PATCH.mockResolvedValueOnce(refused(503));
    let down: Awaited<ReturnType<typeof hook.result.current.update>> | undefined;
    await act(async () => {
      down = await hook.result.current.update("p-aaaaaaaaaaaa", { revision: "1", title: "x" }, ctx);
    });
    expect(down).toMatchObject({ ok: false, failure: { reread: true } });
    await waitFor(() => expect(client.GET.mock.calls.length).toBe(before + 2));
  });

  it("a lost connection is a write whose outcome is not known: the list is read again", async () => {
    const hook = await ready();
    client.POST.mockRejectedValueOnce(new TypeError("network"));
    const before = client.GET.mock.calls.length;
    let result: Awaited<ReturnType<typeof hook.result.current.create>> | undefined;
    await act(async () => {
      result = await hook.result.current.create(
        { name: "a", title: "A", body: "x" },
        { name: "a", body: "x", ids: [] },
      );
    });
    expect(result).toMatchObject({ ok: false, failure: { reread: true } });
    await waitFor(() => expect(client.GET.mock.calls.length).toBe(before + 1));
  });

  it("delete sends the revision as a query and takes the prompt out of the list", async () => {
    const hook = await ready();
    client.DELETE.mockResolvedValueOnce({
      response: { status: 204, ok: true, headers: new Headers() },
    });
    await act(async () => {
      await hook.result.current.remove("p-aaaaaaaaaaaa", "1");
    });
    expect(client.DELETE).toHaveBeenCalledWith("/api/prompts/{promptId}", {
      params: { path: { promptId: "p-aaaaaaaaaaaa" }, query: { revision: "1" } },
    });
    expect(prompts(hook)).toHaveLength(0);
  });

  it("404 delete is gone: the prompt leaves the list and it is a success", async () => {
    const hook = await ready();
    client.DELETE.mockResolvedValueOnce(refused(404));
    let result: Awaited<ReturnType<typeof hook.result.current.remove>> | undefined;
    await act(async () => {
      result = await hook.result.current.remove("p-aaaaaaaaaaaa", "1");
    });
    expect(result?.ok).toBe(true);
    expect(prompts(hook)).toHaveLength(0);
  });

  it("a stale delete is told and the list is read again", async () => {
    const hook = await ready();
    client.DELETE.mockResolvedValueOnce(
      refused(409, { title: "x", status: 409, code: "stale_revision" }),
    );
    const before = client.GET.mock.calls.length;
    let result: Awaited<ReturnType<typeof hook.result.current.remove>> | undefined;
    await act(async () => {
      result = await hook.result.current.remove("p-aaaaaaaaaaaa", "1");
    });
    expect(result).toMatchObject({ ok: false, failure: { reread: true } });
    expect(prompts(hook)).toHaveLength(1);
    await waitFor(() => expect(client.GET.mock.calls.length).toBe(before + 1));
  });

  it("get reads the whole prompt by its id, and says a 404 is gone", async () => {
    const hook = await ready();
    client.GET.mockResolvedValueOnce({
      data: full(),
      response: { status: 200, ok: true, headers: new Headers() },
    });
    const found = await hook.result.current.get("p-aaaaaaaaaaaa");
    expect(found).toMatchObject({ ok: true, value: { body: "Hello" } });
    expect(client.GET).toHaveBeenLastCalledWith("/api/prompts/{promptId}", {
      params: { path: { promptId: "p-aaaaaaaaaaaa" } },
    });
    client.GET.mockResolvedValueOnce(refused(404));
    const gone = await hook.result.current.get("p-aaaaaaaaaaaa");
    expect(gone).toMatchObject({ ok: false, failure: { gone: true } });
  });
});

describe("what stays on the device", () => {
  it("nothing is written to localStorage or sessionStorage", async () => {
    const keep = vi.spyOn(Storage.prototype, "setItem");
    const hook = await ready();
    client.POST.mockResolvedValueOnce({
      data: full({ id: "p-cccccccccccc", name: "new" }),
      response: { status: 201, ok: true, headers: new Headers() },
    });
    await act(async () => {
      await hook.result.current.create(
        { name: "new", title: "N", body: "x" },
        { name: "new", body: "x", ids: [] },
      );
    });
    expect(keep).not.toHaveBeenCalled();
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
    keep.mockRestore();
  });
});
