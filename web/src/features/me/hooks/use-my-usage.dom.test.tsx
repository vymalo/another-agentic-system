// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiMyUsage } from "@/lib/api/types";

/*
 * `GET /api/me/usage?month=` (ADR 0065) as the page reads it: only `month` is sent (never an `owner`), a 404 is an
 * orchestrator from before the ledger, a 401 is a signed-out person, and an answer that is no longer the one asked
 * for (the month moved on, or it names another month) is never shown.
 */
const client = vi.hoisted(() => ({ GET: vi.fn() }));
vi.mock("@/lib/api/client", async () => ({
  ...(await vi.importActual<typeof import("@/lib/api/client")>("@/lib/api/client")),
  api: client,
}));

import { useMyUsage } from "./use-my-usage";

const usage = (month: string): ApiMyUsage => ({
  month,
  currency: "EUR",
  prices: null,
  tasks: 1,
  models: [],
  sandbox: [],
  unmeasured: [],
  total: {
    amount: "0.000000",
    currency: "EUR",
    complete: true,
    unpriced: { models: [], sandbox: 0, unmeasured: 0 },
  },
});

const answer = (status: number, data?: unknown, error?: unknown) => ({
  data,
  error,
  response: { status },
});

type Init = { params: { query: { month: string } } };

const deferred = <T,>() => {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
};

beforeEach(() => {
  client.GET.mockReset();
});
afterEach(cleanup);

describe("useMyUsage", () => {
  it("sends only `month`: the query is exactly {month}, no owner", async () => {
    client.GET.mockResolvedValue(answer(200, usage("2026-10")));
    const { result } = renderHook(() => useMyUsage("2026-10"));
    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(client.GET).toHaveBeenCalledTimes(1);
    const [path, init] = client.GET.mock.calls[0] as [string, { params: { query: unknown } }];
    expect(path).toBe("/api/me/usage");
    expect(init.params.query).toEqual({ month: "2026-10" });
    expect(Object.keys(init)).toEqual(["params"]);
  });

  it("a 404 is `unavailable`: an orchestrator with no such route, not a failure", async () => {
    client.GET.mockResolvedValue(answer(404, undefined, { title: "Not Found", status: 404 }));
    const { result } = renderHook(() => useMyUsage("2026-10"));
    await waitFor(() => expect(result.current.status).toBe("unavailable"));
  });

  it("a 401 is `signed-out`, not an error line", async () => {
    client.GET.mockResolvedValue(answer(401, undefined, { title: "Unauthorized", status: 401 }));
    const { result } = renderHook(() => useMyUsage("2026-10"));
    await waitFor(() => expect(result.current.status).toBe("signed-out"));
  });

  it("an error says the problem's words", async () => {
    client.GET.mockResolvedValue(
      answer(503, undefined, { title: "Unavailable", status: 503, detail: "the ledger is down" }),
    );
    const { result } = renderHook(() => useMyUsage("2026-10"));
    await waitFor(() => expect(result.current.status).toBe("error"));
    expect(result.current).toMatchObject({ message: "the ledger is down" });
  });

  it("an error body that is not JSON still says something sensible, with the status", async () => {
    client.GET.mockResolvedValue(answer(502, undefined, "<html>Bad Gateway</html>"));
    const { result } = renderHook(() => useMyUsage("2026-10"));
    await waitFor(() => expect(result.current.status).toBe("error"));
    const message = (result.current as { message: string }).message;
    expect(message).toContain("502");
    expect(message).not.toContain("<html>");
  });

  it("a read that throws is an error line", async () => {
    client.GET.mockRejectedValue(new Error("network down"));
    const { result } = renderHook(() => useMyUsage("2026-10"));
    await waitFor(() => expect(result.current.status).toBe("error"));
    expect(result.current).toMatchObject({ message: "network down" });
  });

  it("a stale answer is dropped: the month moved on before it arrived, and the new month is still loading", async () => {
    const slow = deferred<ReturnType<typeof answer>>();
    const quick = deferred<ReturnType<typeof answer>>();
    client.GET.mockImplementation((_path: string, init: Init) =>
      init.params.query.month === "2026-09" ? slow.promise : quick.promise,
    );
    const { result, rerender } = renderHook(({ month }) => useMyUsage(month), {
      initialProps: { month: "2026-09" },
    });
    rerender({ month: "2026-10" });
    await act(async () => {
      slow.resolve(answer(200, usage("2026-09")));
    });
    // the answer for September arrived, and October is still waiting for its own
    expect(result.current.status).toBe("loading");
    await act(async () => {
      quick.resolve(answer(200, usage("2026-10")));
    });
    expect(result.current).toMatchObject({ status: "ready", usage: { month: "2026-10" } });
  });

  it("a stale answer that arrives after the new month's own does not replace it", async () => {
    const slow = deferred<ReturnType<typeof answer>>();
    client.GET.mockImplementation((_path: string, init: Init) =>
      init.params.query.month === "2026-09"
        ? slow.promise
        : Promise.resolve(answer(200, usage("2026-10"))),
    );
    const { result, rerender } = renderHook(({ month }) => useMyUsage(month), {
      initialProps: { month: "2026-09" },
    });
    rerender({ month: "2026-10" });
    await waitFor(() => expect(result.current.status).toBe("ready"));
    await act(async () => {
      slow.resolve(answer(200, usage("2026-09")));
    });
    expect(result.current).toMatchObject({ status: "ready", usage: { month: "2026-10" } });
  });

  it("a stale error is dropped too", async () => {
    const slow = deferred<ReturnType<typeof answer>>();
    client.GET.mockImplementation((_path: string, init: Init) =>
      init.params.query.month === "2026-09" ? slow.promise : new Promise(() => {}),
    );
    const { result, rerender } = renderHook(({ month }) => useMyUsage(month), {
      initialProps: { month: "2026-09" },
    });
    rerender({ month: "2026-10" });
    await act(async () => {
      slow.resolve(answer(500, undefined, { title: "boom", status: 500 }));
    });
    expect(result.current.status).toBe("loading");
  });

  it("the render right after the month changes is loading, never the last month's figures", async () => {
    client.GET.mockImplementation((_path: string, init: Init) =>
      Promise.resolve(answer(200, usage(init.params.query.month))),
    );
    const seen: string[] = [];
    const { result, rerender } = renderHook(
      ({ month }) => {
        const read = useMyUsage(month);
        seen.push(
          `${month}:${read.status}${read.status === "ready" ? `:${read.usage.month}` : ""}`,
        );
        return read;
      },
      { initialProps: { month: "2026-09" } },
    );
    await waitFor(() => expect(result.current.status).toBe("ready"));
    seen.length = 0;
    rerender({ month: "2026-10" });
    await waitFor(() => expect(result.current).toMatchObject({ usage: { month: "2026-10" } }));
    // no render pairs October with September's answer
    expect(seen.filter((s) => s.startsWith("2026-10:ready:2026-09"))).toEqual([]);
    expect(seen[0]).toBe("2026-10:loading");
  });

  it("an answer for another month than the one asked is an error, not that month's figures", async () => {
    client.GET.mockResolvedValue(answer(200, usage("2026-08")));
    const { result } = renderHook(() => useMyUsage("2026-10"));
    await waitFor(() => expect(result.current.status).toBe("error"));
  });

  it("reload asks again, and the answer replaces the error", async () => {
    client.GET.mockResolvedValueOnce(answer(500, undefined, { title: "boom", status: 500 }));
    const { result } = renderHook(() => useMyUsage("2026-10"));
    await waitFor(() => expect(result.current.status).toBe("error"));
    client.GET.mockResolvedValueOnce(answer(200, usage("2026-10")));
    act(() => result.current.reload());
    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(client.GET).toHaveBeenCalledTimes(2);
  });
});
