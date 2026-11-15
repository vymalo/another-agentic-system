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
      new Promise((resolve) => {
        calls.push({
          path,
          body: init.body,
          signal: init.signal,
          resolve: resolve as Call["resolve"],
        });
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
});
