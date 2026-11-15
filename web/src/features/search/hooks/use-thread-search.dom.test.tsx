// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiThread } from "@/lib/api/types";
import { DEBOUNCE_MS, useThreadSearch } from "./use-thread-search";

/*
 * The search's requests (ADR 0068): 250 ms after the last key, the text only in the body, one AbortController per request, an answer
 * for an earlier text dropped, a 429 tried again after its Retry-After, a 503 told. The API client is replaced, so each answer is
 * the test's own and its timing too.
 */

type Call = {
  path: string;
  body: { q: string; limit: number; archived: string; cursor?: string };
  signal: AbortSignal;
  resolve: (r: { data?: unknown; response: { status: number; headers: Headers } }) => void;
};
let calls: Call[] = [];

vi.mock("@/lib/api/client", () => ({
  api: {
    POST: (path: string, init: { body: Call["body"]; signal: AbortSignal }) =>
      new Promise((resolve, reject) => {
        calls.push({
          path,
          body: init.body,
          signal: init.signal,
          resolve: resolve as Call["resolve"],
        });
        // a real fetch rejects when its signal aborts
        init.signal.addEventListener("abort", () =>
          reject(new DOMException("The operation was aborted.", "AbortError")),
        );
      }),
  },
}));
vi.mock("@/features/chat/hooks/use-ui-config", () => ({
  useSearchConfig: () => ({ maxQueryLength: 10, maxTerms: 2 }),
}));

const thread = (id: string): ApiThread => ({
  id,
  owner: "dev@example.com",
  title: id,
  target: { agentId: "coder" },
  state: "done",
  createdAt: "2026-10-02T10:00:00Z",
  updatedAt: "2026-10-02T10:00:00Z",
  lastSeq: 3,
});
const snippet = { text: "login", cutStart: false, cutEnd: false, matches: [{ start: 0, end: 5 }] };
const page = (ids: string[], next?: string) => ({
  data: {
    query: { terms: ["login"] },
    results: ids.map((id) => ({
      thread: thread(id),
      rank: 2,
      messageHits: 0,
      hits: [{ field: "title", snippet }],
    })),
    ...(next ? { next } : {}),
  },
  response: { status: 200, headers: new Headers() },
});
const failure = (status: number, headers: Record<string, string> = {}) => ({
  response: { status, headers: new Headers(headers) },
});

beforeEach(() => {
  calls = [];
  vi.useFakeTimers();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const type = (result: { current: ReturnType<typeof useThreadSearch> }, text: string) =>
  act(() => result.current.setValue(text));
const wait = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

describe("useThreadSearch", () => {
  it("waits 250 ms after the last key and sends the text in the body of a POST", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "lo");
    await wait(DEBOUNCE_MS - 1);
    type(result, "log");
    await wait(DEBOUNCE_MS - 1);
    expect(calls).toHaveLength(0);
    await wait(1);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.path).toBe("/api/search/threads");
    expect(calls[0]?.body).toEqual({ q: "log", limit: 20, archived: "include" });
    expect(result.current.status).toBe("searching");
  });

  it("draws the answer, and says how many chats it found", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(page(["a", "b", "c"])));
    expect(result.current.status).toBe("ready");
    expect(result.current.page?.results).toHaveLength(3);
    expect(result.current.options).toHaveLength(3);
    expect(result.current.announcement).toBe("3 chats found");
  });

  it("aborts the request in flight when the text changes, and drops its answer if it comes anyway", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "lo");
    await wait(DEBOUNCE_MS);
    const first = calls[0];
    type(result, "login");
    await wait(DEBOUNCE_MS);
    expect(first?.signal.aborted).toBe(true);
    // the second answers first; the first, late, must not replace it
    await act(async () => calls[1]?.resolve(page(["new"])));
    await act(async () => first?.resolve(page(["old"])));
    expect(result.current.page?.results.map((r) => r.thread.id)).toEqual(["new"]);
  });

  it("keeps the last answer, to dim it, while the next is fetched", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(page(["a"])));
    type(result, "login p");
    expect(result.current.status).toBe("searching");
    expect(result.current.page?.results).toHaveLength(1);
  });

  it("returns to the rail, and sends nothing, when the box is blank", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    type(result, "  \t");
    await wait(DEBOUNCE_MS * 2);
    expect(calls).toHaveLength(0);
    expect(result.current.active).toBe(false);
    expect(result.current.status).toBe("idle");
  });

  it("clips the query to the limit the server states, in code points", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "😀".repeat(12));
    await wait(DEBOUNCE_MS);
    expect(calls[0]?.body.q).toBe("😀".repeat(10));
  });

  it("asks for the next page with the cursor and adds it", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(page(["a"], "CUR")));
    expect(result.current.announcement).toBe("1 or more chats found");
    act(() => result.current.more());
    expect(calls[1]?.body.cursor).toBe("CUR");
    expect(calls[1]?.body.q).toBe("login");
    await act(async () => calls[1]?.resolve(page(["b"])));
    expect(result.current.page?.results.map((r) => r.thread.id)).toEqual(["a", "b"]);
    expect(result.current.page?.next).toBeUndefined();
  });

  it("tries again after a 429's Retry-After, and not before", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(failure(429, { "Retry-After": "2" })));
    expect(result.current.status).toBe("limited");
    expect(result.current.seconds).toBe(2);
    await wait(1_999);
    expect(calls).toHaveLength(1);
    await wait(1);
    expect(calls).toHaveLength(2);
  });

  it("tells a 503 and a failure, and Retry sends the text again", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(failure(503, { "Retry-After": "1" })));
    expect(result.current.status).toBe("busy");
    act(() => result.current.retry());
    await act(async () => calls[1]?.resolve(failure(500)));
    expect(result.current.status).toBe("error");
    act(() => result.current.retry());
    await act(async () => calls[2]?.resolve(page(["a"])));
    expect(result.current.status).toBe("ready");
  });

  it("says when more words were typed than the server uses", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "one two three");
    await wait(DEBOUNCE_MS);
    await act(async () =>
      calls[0]?.resolve({
        data: { query: { terms: ["one", "two"] }, results: [] },
        response: { status: 200, headers: new Headers() },
      }),
    );
    expect(result.current.truncated).toBe(2);
  });

  it("never shows an aborted request as an error (9)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "lo");
    await wait(DEBOUNCE_MS);
    type(result, "login");
    await wait(DEBOUNCE_MS);
    expect(calls[0]?.signal.aborted).toBe(true);
    expect(result.current.status).toBe("searching");
  });

  it("replaces the results for a new query, with no cursor, and Show more sends the page's own query (10)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(page(["a"], "CUR")));
    type(result, "logout");
    await wait(DEBOUNCE_MS);
    expect(calls[1]?.body).toEqual({ q: "logout", limit: 20, archived: "include" });
    await act(async () => calls[1]?.resolve(page(["b"], "CUR2")));
    expect(result.current.page?.results.map((r) => r.thread.id)).toEqual(["b"]);
    act(() => result.current.more());
    expect(calls[2]?.body).toMatchObject({ q: "logout", cursor: "CUR2" });
  });

  it("a cleared box drops the answer that was on its way (11)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "pla");
    await wait(DEBOUNCE_MS);
    type(result, "");
    await act(async () => calls[0]?.resolve(page(["late"])));
    expect(result.current.page).toBeNull();
    expect(result.current.status).toBe("idle");
  });

  it("Show more during the debounce of a newer text does not cancel that text's search (1)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "pla");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(page(["a"], "CUR")));
    type(result, "plan");
    expect(result.current.canMore).toBe(false);
    act(() => result.current.more());
    expect(calls).toHaveLength(1);
    await wait(DEBOUNCE_MS);
    expect(calls[1]?.body.q).toBe("plan");
    expect(calls[1]?.body.cursor).toBeUndefined();
  });

  it("an answer for the text that was replaced is dropped at once, not drawn during the debounce (2)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "pla");
    await wait(DEBOUNCE_MS);
    type(result, "plan");
    expect(calls[0]?.signal.aborted).toBe(true);
    await act(async () => calls[0]?.resolve(page(["old"])));
    expect(result.current.page).toBeNull();
    expect(result.current.status).toBe("searching");
  });

  it("only a settled page for the text in the box drives the list: errors, a new text and no match leave it inert (3)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(page(["a"])));
    expect(result.current.listed).toBe(true);
    act(() => result.current.move(1));
    expect(result.current.activeId).toBeDefined();
    type(result, "login x");
    expect(result.current.listed).toBe(false);
    expect(result.current.activeId).toBeUndefined();
    await wait(DEBOUNCE_MS);
    await act(async () => calls[1]?.resolve(failure(500)));
    expect(result.current.listed).toBe(false);
    act(() => result.current.move(1));
    expect(result.current.activeId).toBeUndefined();
    act(() => result.current.retry());
    await act(async () =>
      calls[2]?.resolve({
        data: { query: { terms: ["login"] }, results: [] },
        response: { status: 200, headers: new Headers() },
      }),
    );
    expect(result.current.listed).toBe(false);
  });

  it("a failed Show more keeps the hits, and Retry sends the same page again (4)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "pla");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(page(["a"], "CUR")));
    act(() => result.current.more());
    await act(async () => calls[1]?.resolve(failure(500)));
    expect(result.current.status).toBe("error");
    expect(result.current.failedMore).toBe(true);
    expect(result.current.listed).toBe(true);
    expect(result.current.page?.results).toHaveLength(1);
    act(() => result.current.retry());
    expect(calls[2]?.body).toMatchObject({ q: "pla", cursor: "CUR" });
    await act(async () => calls[2]?.resolve(page(["b"])));
    expect(result.current.page?.results.map((r) => r.thread.id)).toEqual(["a", "b"]);
  });

  it("caps a Retry-After at 30 s, and reads an HTTP date (5)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(failure(429, { "Retry-After": "3600" })));
    expect(result.current.seconds).toBe(30);
  });

  it("tries a 429 three times by itself and then waits for the person (5)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    for (let i = 0; i < 4; i++) {
      await act(async () => calls[i]?.resolve(failure(429, { "Retry-After": "1" })));
      await wait(1_000);
    }
    expect(calls).toHaveLength(4);
    expect(result.current.exhausted).toBe(true);
    await wait(60_000);
    expect(calls).toHaveLength(4);
    act(() => result.current.retry());
    expect(calls).toHaveLength(5);
  });

  it("forgets a pending 429 retry when the text changes or the box is cleared (5)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(failure(429, { "Retry-After": "2" })));
    type(result, "");
    await wait(5_000);
    expect(calls).toHaveLength(1);
  });

  it("never retries a 503 by itself (5)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(failure(503, { "Retry-After": "1" })));
    await wait(5_000);
    expect(calls).toHaveLength(1);
    expect(result.current.status).toBe("busy");
  });

  it("treats an answer of the wrong shape as an error instead of throwing (6)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    const bad = page(["a"]);
    const first = bad.data.results[0];
    if (first) first.hits = [{ field: "title", snippet: { ...snippet, matches: null } }] as never;
    await act(async () => calls[0]?.resolve(bad));
    expect(result.current.status).toBe("error");
    expect(result.current.page).toBeNull();
  });

  it("keeps a thread once when two pages both have it (6)", async () => {
    const { result } = renderHook(() => useThreadSearch());
    type(result, "login");
    await wait(DEBOUNCE_MS);
    await act(async () => calls[0]?.resolve(page(["a", "b"], "CUR")));
    act(() => result.current.more());
    await act(async () => calls[1]?.resolve(page(["b", "c"])));
    expect(result.current.page?.results.map((r) => r.thread.id)).toEqual(["a", "b", "c"]);
  });
});
