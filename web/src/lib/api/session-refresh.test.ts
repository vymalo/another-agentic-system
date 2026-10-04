// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { navigation } from "./session";
import {
  FOCUS_GAP_MS,
  KEEP_WARM_MS,
  keepSessionWarm,
  PARK_MS,
  renewSession,
  resetSessionState,
  sessionStatus,
  subscribeSession,
  withSessionRefresh,
} from "./session-refresh";

const json = (status = 200) => new Response("{}", { status });
const unauthorized = () => new Response("Unauthorized", { status: 401 });
const get = (path = "/api/threads", init?: RequestInit) =>
  new Request(`http://app.test${path}`, init);

/** The edge, as the browser asks it: what each question is answered with, in order (the last one repeats). */
function edge(...answers: Array<() => Response | Promise<Response>>) {
  const asked: string[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    asked.push(String(input));
    expect(init?.credentials).toBe("same-origin");
    expect(init?.cache).toBe("no-store");
    const answer = answers[Math.min(asked.length - 1, answers.length - 1)];
    return (answer as () => Response)();
  });
  vi.stubGlobal("fetch", fetch);
  return asked;
}

/** An orchestrator behind the edge: each call is answered by the next function (the last one repeats). */
function api(...answers: Array<(request: Request) => Response | Promise<Response>>) {
  const seen: Array<{ method: string; path: string; body: string }> = [];
  const send = vi.fn(async (request: Request) => {
    seen.push({
      method: request.method,
      path: new URL(request.url).pathname,
      body: request.method === "POST" ? await request.clone().text() : "",
    });
    const answer = answers[Math.min(seen.length - 1, answers.length - 1)];
    return (answer as (r: Request) => Response)(request);
  });
  return { send, seen };
}

let go: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  resetSessionState();
  vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
  go = vi.spyOn(navigation, "go").mockImplementation(() => {});
  window.history.pushState({}, "", "/threads/abc");
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("a 401 and a session that can be refreshed", () => {
  it("asks the edge once, sends the request again and returns that answer: no banner, no redirect", async () => {
    const asked = edge(() => json());
    const { send, seen } = api(unauthorized, () => json());
    const res = await withSessionRefresh(send)(get());
    expect(res.status).toBe(200);
    // the refresh is the edge's own userinfo, derived from the sign-in's prefix
    expect(asked).toEqual(["/oauth2/userinfo"]);
    expect(seen.map((s) => s.path)).toEqual(["/api/threads", "/api/threads"]);
    expect(sessionStatus()).toBe("ok");
    expect(go).not.toHaveBeenCalled();
  });

  it("sends a POST again with its body whole", async () => {
    edge(() => json());
    const { send, seen } = api(unauthorized, () => json());
    const body = JSON.stringify({ threadId: "t", messages: [{ role: "user", content: "hi" }] });
    const res = await withSessionRefresh(send)(
      get("/agui/agents/coder", {
        method: "POST",
        body,
        headers: { "content-type": "application/json" },
      }),
    );
    expect(res.status).toBe(200);
    expect(seen.map((s) => [s.method, s.body])).toEqual([
      ["POST", body],
      ["POST", body],
    ]);
  });

  it("does nothing for an answer that is not a 401, and does not ask the edge", async () => {
    const asked = edge(() => json());
    const { send } = api(() => json(500));
    const res = await withSessionRefresh(send)(get());
    expect(res.status).toBe(500);
    expect(asked).toEqual([]);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("asks once for any number of requests that meet the 401 together", async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => {
      release = r;
    });
    const asked = edge(async () => {
      await gate;
      return json();
    });
    const { send } = api(unauthorized, unauthorized, unauthorized, () => json());
    const wrapped = withSessionRefresh(send);
    const all = Promise.all([
      wrapped(get("/api/a")),
      wrapped(get("/api/b")),
      wrapped(get("/api/c")),
    ]);
    await vi.waitFor(() => expect(asked).toHaveLength(1));
    release();
    expect((await all).map((r) => r.status)).toEqual([200, 200, 200]);
    expect(asked).toHaveLength(1);
  });

  it("goes again only once: a request that is still a 401 after a refresh says the session has ended", async () => {
    edge(() => json());
    const { send } = api(unauthorized);
    void withSessionRefresh(send)(get());
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
    // two calls: the first and the one after the refresh, never a third
    expect(send).toHaveBeenCalledTimes(2);
    expect(go).not.toHaveBeenCalled();
  });

  it("a refresh the edge cannot answer is not a verdict: the 401 stands, nothing is asked of the person", async () => {
    edge(() => json(502));
    const { send } = api(unauthorized);
    const res = await withSessionRefresh(send)(get());
    expect(res.status).toBe(401);
    expect(sessionStatus()).toBe("ok");
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("a network error on the refresh is the same", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("network");
      }),
    );
    const { send } = api(unauthorized);
    const res = await withSessionRefresh(send)(get());
    expect(res.status).toBe(401);
    expect(sessionStatus()).toBe("ok");
  });
});

describe("a 401 and no session", () => {
  it("says the session has ended, holds the request and sends it when the person is back", async () => {
    const asked = edge(unauthorized);
    const { send, seen } = api(unauthorized, () => json());
    const changes: string[] = [];
    subscribeSession(() => changes.push(sessionStatus()));
    let settled = false;
    const pending = withSessionRefresh(send)(get()).then((r) => {
      settled = true;
      return r;
    });
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
    await new Promise((r) => setTimeout(r, 20));
    expect(settled).toBe(false);
    expect(seen).toHaveLength(1);
    // no redirect, ever, from here: the page is kept
    expect(go).not.toHaveBeenCalled();

    // the person signs in (the popup, a tab): the edge now has a session
    edge(() => json());
    expect(await renewSession()).toBe("alive");
    const res = await pending;
    expect(res.status).toBe(200);
    expect(sessionStatus()).toBe("ok");
    expect(changes).toEqual(["ended", "ok"]);
    expect(seen).toHaveLength(2);
    expect(asked).toEqual(["/oauth2/userinfo"]);
  });

  it("a request that meets a session already known to have ended waits without asking again", async () => {
    const asked = edge(unauthorized);
    const { send } = api(unauthorized, unauthorized, () => json());
    const wrapped = withSessionRefresh(send);
    void wrapped(get("/api/a"));
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
    void wrapped(get("/api/b"));
    await new Promise((r) => setTimeout(r, 20));
    expect(asked).toHaveLength(1);
    expect(send).toHaveBeenCalledTimes(2);
  });

  it("lets the 401 through when nobody signs in for PARK_MS: the caller's error line, as before", async () => {
    vi.useFakeTimers();
    edge(unauthorized);
    const { send } = api(unauthorized);
    const pending = withSessionRefresh(send)(get());
    await vi.advanceTimersByTimeAsync(0);
    expect(sessionStatus()).toBe("ended");
    await vi.advanceTimersByTimeAsync(PARK_MS);
    expect((await pending).status).toBe(401);
    expect(go).not.toHaveBeenCalled();
  });

  it("a request that is aborted while it waits is let go", async () => {
    edge(unauthorized);
    const { send } = api(unauthorized);
    const controller = new AbortController();
    const pending = withSessionRefresh(send)(get("/api/threads", { signal: controller.signal }));
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
    controller.abort();
    expect((await pending).status).toBe(401);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("a redirect to the sign-in is no session either (the edge's own answer to a browser)", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: false, status: 0, type: "opaqueredirect" }) as unknown as Response),
    );
    const { send } = api(unauthorized);
    void withSessionRefresh(send)(get());
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
  });
});

describe("without the edge's sign-in", () => {
  it("is the fetch it wraps: a 401 is a 401, nothing is asked, nothing waits", async () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "");
    const asked = edge(() => json());
    const { send } = api(unauthorized);
    const res = await withSessionRefresh(send)(get());
    expect(res.status).toBe(401);
    expect(asked).toEqual([]);
    expect(sessionStatus()).toBe("ok");
    expect(go).not.toHaveBeenCalled();
  });

  it("keeping warm is nothing", () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "");
    vi.useFakeTimers();
    const asked = edge(() => json());
    const stop = keepSessionWarm();
    vi.advanceTimersByTime(KEEP_WARM_MS * 3);
    stop();
    expect(asked).toEqual([]);
  });
});

describe("keeping the session warm", () => {
  it("asks the edge on an interval shorter than oauth2-proxy's cookie-refresh, and stops when told", async () => {
    // the chart's `--cookie-refresh=10m`: a ping is a refresh only if it comes after that, and it must come before the token's end
    expect(KEEP_WARM_MS).toBeLessThan(10 * 60_000);
    vi.useFakeTimers();
    const asked = edge(() => json());
    const stop = keepSessionWarm();
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS - 1);
    expect(asked).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(asked).toEqual(["/oauth2/userinfo"]);
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS);
    expect(asked).toHaveLength(2);
    stop();
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS * 2);
    expect(asked).toHaveLength(2);
  });

  it("asks when the window comes back after a while, not on every focus", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-04T10:00:00Z"));
    const asked = edge(() => json());
    const stop = keepSessionWarm();
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(0);
    expect(asked).toHaveLength(0); // it was asked a moment ago, by being open
    vi.setSystemTime(Date.now() + FOCUS_GAP_MS + 1);
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(0);
    expect(asked).toHaveLength(1);
    stop();
  });

  it("a session that is gone is what the page is told, and never a redirect", async () => {
    vi.useFakeTimers();
    edge(unauthorized);
    const stop = keepSessionWarm();
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS);
    expect(sessionStatus()).toBe("ended");
    expect(go).not.toHaveBeenCalled();
    stop();
  });
});
