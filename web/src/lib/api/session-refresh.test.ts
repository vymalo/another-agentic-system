// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { navigation } from "./session";
import {
  endedAgainSoon,
  FOCUS_GAP_MS,
  IDLE_LIMIT_MS,
  KEEP_WARM_MS,
  keepSessionWarm,
  PARK_MS,
  renewSession,
  resetSessionState,
  SessionChangedError,
  sessionStatus,
  subscribeSession,
  watchForSignIn,
  withSessionRefresh,
} from "./session-refresh";

const json = (status = 200) => new Response("{}", { status });
/** The edge's userinfo for a person. */
const person = (email: string) => () =>
  new Response(JSON.stringify({ user: email, email }), { status: 200 });
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
      // reads it as fetch does: a body can be sent once
      body: request.method === "POST" ? await request.text() : "",
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

describe("a request is sent up to three times", () => {
  it("a POST refused, refreshed, refused again, held and sent after the sign-in carries its whole body each time", async () => {
    edge(() => json());
    const { send, seen } = api(unauthorized, unauthorized, () => json());
    const body = JSON.stringify({
      threadId: "t",
      messages: [{ role: "user", content: "hello there" }],
    });
    const pending = withSessionRefresh(send)(
      get("/agui/agents/coder", {
        method: "POST",
        body,
        headers: { "content-type": "application/json" },
      }),
    );
    // the refresh said alive and the call was refused again: the person is asked, the call is held
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
    expect(seen).toHaveLength(2);
    // the person signs in
    expect(await renewSession()).toBe("alive");
    const res = await pending;
    expect(res.status).toBe(200);
    expect(seen.map((s) => [s.method, s.body])).toEqual([
      ["POST", body],
      ["POST", body],
      ["POST", body],
    ]);
  });

  it("lets go of the body of a 401 it is about to send again", async () => {
    edge(() => json());
    const refused = unauthorized();
    const cancel = vi.spyOn(refused.body as ReadableStream, "cancel");
    const { send } = api(
      () => refused,
      () => json(),
    );
    const res = await withSessionRefresh(send)(get());
    expect(res.status).toBe(200);
    expect(cancel).toHaveBeenCalledTimes(1);
  });

  it("hands the caller the 401 it gave up on with its body unread", async () => {
    vi.useFakeTimers();
    edge(unauthorized);
    const { send } = api(() => new Response('{"detail":"sign in"}', { status: 401 }));
    const pending = withSessionRefresh(send)(get());
    await vi.advanceTimersByTimeAsync(PARK_MS);
    const res = await pending;
    expect(res.status).toBe(401);
    expect(await res.text()).toBe('{"detail":"sign in"}');
  });
});

describe("a person who signs in as somebody else", () => {
  it("is sent nothing held for the one before: the call is rejected and the page is read again", async () => {
    const reload = vi.spyOn(navigation, "reload").mockImplementation(() => {});
    // the page has been Alice's: the edge said so
    edge(person("Alice@Example.com"));
    expect(await renewSession()).toBe("alive");
    // her session ends; the call is held, a run's POST
    edge(unauthorized);
    const { send, seen } = api(unauthorized, () => json());
    const pending = withSessionRefresh(send)(
      get("/agui/agents/coder", { method: "POST", body: "{}" }),
    );
    const outcome = pending.then(
      () => "sent",
      (e) => e,
    );
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
    // somebody signs in, as Bob
    edge(person("bob@example.com"));
    expect(await renewSession()).toBe("changed");
    expect(await outcome).toBeInstanceOf(SessionChangedError);
    expect(sessionStatus()).toBe("changed");
    expect(reload).toHaveBeenCalledTimes(1);
    // nothing of Alice's was sent as Bob, and nothing is from now on
    expect(seen).toHaveLength(1);
    await expect(withSessionRefresh(send)(get())).rejects.toBeInstanceOf(SessionChangedError);
    expect(send).toHaveBeenCalledTimes(1);
    // one reload however many ask
    edge(person("bob@example.com"));
    await renewSession();
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("is the same person, however the address is written", async () => {
    const reload = vi.spyOn(navigation, "reload").mockImplementation(() => {});
    edge(person("alice@example.com"));
    await renewSession();
    edge(person(" ALICE@example.com "));
    expect(await renewSession()).toBe("alive");
    expect(reload).not.toHaveBeenCalled();
  });

  it("is told on a refresh too: a call refused, and the edge now has another person's session", async () => {
    vi.spyOn(navigation, "reload").mockImplementation(() => {});
    edge(person("alice@example.com"));
    await renewSession();
    edge(person("bob@example.com"));
    const { send } = api(unauthorized, () => json());
    await expect(withSessionRefresh(send)(get())).rejects.toBeInstanceOf(SessionChangedError);
    expect(send).toHaveBeenCalledTimes(1);
  });
});

describe("the sign-in message", () => {
  it("starts a question after the one on its way: that one started before the cookie was set", async () => {
    let calls = 0;
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => {
      release = r;
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        const n = ++calls;
        if (n === 1) {
          await gate;
          return unauthorized(); // asked before the sign-in
        }
        return json();
      }),
    );
    const stop = watchForSignIn();
    const first = renewSession();
    await vi.waitFor(() => expect(calls).toBe(1));
    const channel = new BroadcastChannel("another-agentic.signed-in");
    channel.postMessage("signed-in");
    channel.close();
    await vi.waitFor(() => expect(calls).toBe(1)); // waits for the first, does not join it
    release();
    expect(await first).toBe("gone");
    await vi.waitFor(() => expect(sessionStatus()).toBe("ok"));
    expect(calls).toBe(2);
    stop();
  });
});

describe("ended again soon after a sign-in", () => {
  it("is known for the length of the pause, and not before a sign-in", async () => {
    edge(unauthorized);
    await renewSession();
    expect(sessionStatus()).toBe("ended");
    expect(endedAgainSoon()).toBe(false);
    edge(() => json());
    await renewSession();
    edge(unauthorized);
    await renewSession();
    expect(sessionStatus()).toBe("ended");
    expect(endedAgainSoon()).toBe(true);
    expect(endedAgainSoon(Date.now() + 30_000)).toBe(false);
  });
});

describe("keeping the session warm", () => {
  it("asks the edge once at the start (whose session it is), then on an interval shorter than oauth2-proxy's cookie-refresh, and stops when told", async () => {
    // the chart's `--cookie-refresh=10m`: a ping is a refresh only if it comes after that, and it must come before the token's end
    expect(KEEP_WARM_MS).toBeLessThan(10 * 60_000);
    vi.useFakeTimers();
    const asked = edge(() => json());
    const stop = keepSessionWarm();
    await vi.advanceTimersByTimeAsync(0);
    expect(asked).toEqual(["/oauth2/userinfo"]);
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS - 1);
    expect(asked).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(asked).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS);
    expect(asked).toHaveLength(3);
    stop();
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS * 2);
    expect(asked).toHaveLength(3);
  });

  it("asks when the window comes back after a while, not on every focus", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-04T10:00:00Z"));
    const asked = edge(() => json());
    const stop = keepSessionWarm();
    await vi.advanceTimersByTimeAsync(0);
    expect(asked).toHaveLength(1);
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(0);
    expect(asked).toHaveLength(1); // it was asked a moment ago
    vi.setSystemTime(Date.now() + FOCUS_GAP_MS + 1);
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(0);
    expect(asked).toHaveLength(2);
    stop();
  });

  it("stops asking when nobody has touched the page for IDLE_LIMIT_MS, and asks again when they do", async () => {
    vi.useFakeTimers();
    const asked = edge(() => json());
    const stop = keepSessionWarm();
    await vi.advanceTimersByTimeAsync(0);
    // used: a key now and then keeps it going
    for (let spent = 0; spent < IDLE_LIMIT_MS; spent += KEEP_WARM_MS) {
      await vi.advanceTimersByTimeAsync(KEEP_WARM_MS);
      window.dispatchEvent(new Event("keydown"));
    }
    const whileUsed = asked.length;
    expect(whileUsed).toBeGreaterThan(IDLE_LIMIT_MS / KEEP_WARM_MS - 1);
    // untouched: the interval goes on, and asks nothing, once the limit is passed
    await vi.advanceTimersByTimeAsync(IDLE_LIMIT_MS + KEEP_WARM_MS);
    const untilLimit = asked.length;
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS * 5);
    expect(asked).toHaveLength(untilLimit);
    expect(untilLimit - whileUsed).toBeLessThanOrEqual(IDLE_LIMIT_MS / KEEP_WARM_MS);
    // a pointer, and it is warm again at the next interval
    window.dispatchEvent(new Event("pointermove"));
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS);
    expect(asked).toHaveLength(untilLimit + 1);
    stop();
  });

  it("asks nothing while the page is hidden", async () => {
    vi.useFakeTimers();
    const asked = edge(() => json());
    const stop = keepSessionWarm();
    await vi.advanceTimersByTimeAsync(0);
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS * 3);
    expect(asked).toHaveLength(1);
    visibility.mockReturnValue("visible");
    await vi.advanceTimersByTimeAsync(KEEP_WARM_MS);
    expect(asked).toHaveLength(2);
    stop();
  });

  it("a session that is gone is what the page is told, and never a redirect", async () => {
    vi.useFakeTimers();
    edge(unauthorized);
    const stop = keepSessionWarm();
    await vi.advanceTimersByTimeAsync(0);
    expect(sessionStatus()).toBe("ended");
    expect(go).not.toHaveBeenCalled();
    stop();
  });
});
