// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";

/*
 * The list behind the `/` menu: read when the menu opens and on every reopen, page by page, and the
 * whole prompt and its render read only when one is chosen. A 404 on a prompt reads the list again.
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

import { usePromptPicker } from "./use-prompt-picker";

const CONFIG = { maxRenderedBytes: 32768 };

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

const rendered = (text: string, revision = "2") => ({
  data: { text, revision },
  response: { status: 200, ok: true, headers: new Headers() },
});

const deferred = <T,>() => {
  let resolve!: (value: T) => void;
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

const mount = (open: boolean, config: typeof CONFIG | undefined = CONFIG) =>
  renderHook(({ open }: { open: boolean }) => usePromptPicker({ config, open }), {
    initialProps: { open },
  });

const mountNoConfig = (open: boolean) =>
  renderHook(({ open }: { open: boolean }) => usePromptPicker({ config: undefined, open }), {
    initialProps: { open },
  });

describe("reading the list", () => {
  it("asks nothing until the menu opens, and reads again each time it reopens", async () => {
    const hook = mount(false);
    expect(client.GET).not.toHaveBeenCalled();
    expect(hook.result.current.state.status).toBe("idle");

    hook.rerender({ open: true });
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    expect(client.GET).toHaveBeenCalledTimes(1);

    hook.rerender({ open: false });
    hook.rerender({ open: true });
    await waitFor(() => expect(client.GET).toHaveBeenCalledTimes(2));
    expect(hook.result.current.state.status).toBe("ready");
  });

  it("pages until nextCursor is absent, and sorts by name then id", async () => {
    client.GET.mockReset()
      .mockResolvedValueOnce(page([summary({ id: "p-1", name: "b" })], "p-1"))
      .mockResolvedValueOnce(page([summary({ id: "p-2", name: "a" })], "p-2"))
      .mockResolvedValueOnce(page([summary({ id: "p-3", name: "a" })]));
    const hook = mount(true);
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    expect(client.GET).toHaveBeenCalledTimes(3);
    expect(client.GET.mock.calls.map((c) => c[1])).toEqual([
      { params: { query: {} } },
      { params: { query: { cursor: "p-1" } } },
      { params: { query: { cursor: "p-2" } } },
    ]);
    const state = hook.result.current.state;
    if (state.status !== "ready") throw new Error("not ready");
    expect(state.prompts.map((p) => p.id)).toEqual(["p-2", "p-3", "p-1"]);
  });

  it("drops a read that a newer one overtook", async () => {
    const first = deferred<ReturnType<typeof page>>();
    client.GET.mockReset()
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce(page([summary({ id: "p-new", name: "new" })]));
    const hook = mount(true);
    // the first read is in flight; closing and reopening starts the second
    hook.rerender({ open: false });
    hook.rerender({ open: true });
    await waitFor(() => expect(client.GET).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    const state = hook.result.current.state;
    if (state.status !== "ready") throw new Error("not ready");
    expect(state.prompts.map((p) => p.id)).toEqual(["p-new"]);

    // the first read answers last, and is dropped
    await act(async () => {
      first.resolve(page([summary({ id: "p-old", name: "old" })]));
      await first.promise;
    });
    const after = hook.result.current.state;
    if (after.status !== "ready") throw new Error("not ready");
    expect(after.prompts.map((p) => p.id)).toEqual(["p-new"]);
  });

  it("keeps the list on screen when a later read fails, and says it may be out of date", async () => {
    const hook = mount(true);
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    client.GET.mockResolvedValueOnce(refused(503));
    hook.rerender({ open: false });
    hook.rerender({ open: true });
    await waitFor(() => {
      const state = hook.result.current.state;
      expect(state.status === "ready" && state.readError).toBeTruthy();
    });
    const state = hook.result.current.state;
    if (state.status !== "ready") throw new Error("not ready");
    expect(state.prompts).toHaveLength(1);
  });

  it("is off when the deployment has no library, and asks nothing without a configuration", async () => {
    client.GET.mockReset().mockResolvedValue(refused(404));
    const missing = mount(true);
    await waitFor(() => expect(missing.result.current.state.status).toBe("off"));
    cleanup();
    client.GET.mockReset();
    const none = mountNoConfig(true);
    await waitFor(() => expect(none.result.current.state.status).toBe("off"));
    expect(client.GET).not.toHaveBeenCalled();
  });

  it("says when the roles grant nothing, and when the session ended", async () => {
    client.GET.mockReset().mockResolvedValue(refused(403, { code: "no_access" }));
    const noAccess = mount(true);
    await waitFor(() => expect(noAccess.result.current.state.status).toBe("no-access"));
    cleanup();
    client.GET.mockReset().mockResolvedValue(refused(401));
    const signedOut = mount(true);
    await waitFor(() => expect(signedOut.result.current.state.status).toBe("signed-out"));
  });
});

describe("reading and filling one prompt", () => {
  it("reads the whole prompt by id", async () => {
    client.GET.mockResolvedValueOnce(page([summary()])).mockResolvedValueOnce({
      data: full(),
      response: { status: 200, ok: true },
    });
    const hook = mount(true);
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    const read = await hook.result.current.get("p-aaaaaaaaaaaa");
    expect(read).toEqual({ ok: true, value: full() });
    expect(client.GET.mock.calls[1]).toEqual([
      "/api/prompts/{promptId}",
      { params: { path: { promptId: "p-aaaaaaaaaaaa" } } },
    ]);
  });

  it("reads the list again when the prompt is gone", async () => {
    client.GET.mockResolvedValueOnce(page([summary()])).mockResolvedValueOnce(refused(404));
    const hook = mount(true);
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    const listCalls = () =>
      client.GET.mock.calls.filter(([path]) => path === "/api/prompts").length;
    const before = listCalls();
    const read = await act(() => hook.result.current.get("p-aaaaaaaaaaaa"));
    expect(read.ok).toBe(false);
    // the 404 on getPrompt is not the list read: the list is asked for again
    await waitFor(() => expect(listCalls()).toBeGreaterThan(before));
  });

  it("renders with the revision and the answers, and returns the text", async () => {
    client.GET.mockResolvedValueOnce(page([summary()]));
    client.POST.mockResolvedValueOnce(rendered("Review for Security.", "2"));
    const hook = mount(true);
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    const result = await hook.result.current.render(
      "p-aaaaaaaaaaaa",
      "2",
      [{ id: "focus", values: ["security"] }],
      undefined,
      32768,
    );
    expect(result).toEqual({ ok: true, text: "Review for Security.", revision: "2" });
    expect(client.POST.mock.calls[0]).toEqual([
      "/api/prompts/{promptId}/render",
      {
        params: { path: { promptId: "p-aaaaaaaaaaaa" } },
        body: { revision: "2", answers: [{ id: "focus", values: ["security"] }] },
      },
    ]);
  });

  it("says a stale revision without reading the list again", async () => {
    client.GET.mockResolvedValueOnce(page([summary()]));
    client.POST.mockResolvedValueOnce(refused(409, { code: "stale_revision" }));
    const hook = mount(true);
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    const before = client.GET.mock.calls.length;
    const result = await hook.result.current.render("p-aaaaaaaaaaaa", "1", [], undefined, 32768);
    expect(result.ok).toBe(false);
    if (result.ok) throw new Error("expected a refusal");
    expect(result.failure.stale).toBe(true);
    expect(client.GET.mock.calls.length).toBe(before);
  });

  it("reads the list again when a render says the prompt is gone", async () => {
    client.GET.mockResolvedValueOnce(page([summary()])).mockResolvedValueOnce(page([]));
    client.POST.mockResolvedValueOnce(refused(404));
    const hook = mount(true);
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    const result = await hook.result.current.render("p-aaaaaaaaaaaa", "2", [], undefined, 32768);
    expect(result.ok).toBe(false);
    await waitFor(() => expect(client.GET).toHaveBeenCalledTimes(2));
  });

  it("says a store that cannot answer is worth trying again", async () => {
    client.GET.mockResolvedValueOnce(page([summary()]));
    client.POST.mockResolvedValueOnce(refused(503));
    const hook = mount(true);
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    const result = await hook.result.current.render("p-aaaaaaaaaaaa", "2", [], undefined, 32768);
    if (result.ok) throw new Error("expected a refusal");
    expect(result.failure.message).toContain("Try again");
  });
});
