// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBrowserAuth } from "@/lib/auth/config";
import { navigation } from "@/lib/auth/navigation";
import { resetSignInNeed, signInNeed } from "@/lib/auth/sign-in-need";
import { AuthUnavailableError, SessionEndedError } from "@/lib/auth/tokens";

/*
 * The session of a deployment where the web holds its own tokens (ADR 0054): what a ping is, and the
 * banner, the park, the three sends and the person-switch check on top of it. The tokens themselves
 * are `lib/auth`'s (tested there with real keys); here `getAccessToken` is what the test says.
 */
const held = vi.hoisted(() => ({
  getAccessToken: vi.fn(),
  rejected: undefined as string | undefined,
}));
vi.mock("@/lib/auth/tokens", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/auth/tokens")>()),
  getAccessToken: held.getAccessToken,
}));
vi.mock("@/lib/auth/fetch", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/auth/fetch")>()),
  lastRejectedToken: () => held.rejected,
}));

import {
  FOCUS_GAP_MS,
  keepSessionWarm,
  renewSession,
  resetSessionState,
  SessionChangedError,
  sessionStatus,
  watchForSignIn,
  withSessionRefresh,
} from "./session-refresh";

const alice = { accessToken: "a1", claims: { sub: "s-1", email: "Alice@Example.test" } };
const bob = { accessToken: "b1", claims: { sub: "s-2", email: "bob@example.test" } };
const json = (status = 200) => new Response("{}", { status });
const get = (path = "/api/threads") => new Request(`http://app.test${path}`);

function api(...answers: Array<() => Response>) {
  const seen: string[] = [];
  const send = vi.fn(async (request: Request) => {
    seen.push(new URL(request.url).pathname);
    return (answers[Math.min(seen.length - 1, answers.length - 1)] as () => Response)();
  });
  return { send, seen };
}

let reload: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  resetSessionState();
  held.getAccessToken.mockReset();
  held.rejected = undefined;
  resetSignInNeed();
  setBrowserAuth({ issuer: "https://id.example", clientId: "web", scope: "openid" });
  reload = vi.spyOn(navigation, "reload").mockImplementation(() => {});
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("a 401 in browser mode", () => {
  it("is a token refresh: the refused token is not handed out again, and the request goes again", async () => {
    held.rejected = "a0";
    held.getAccessToken.mockResolvedValue(alice);
    const { send, seen } = api(
      () => json(401),
      () => json(),
    );
    const res = await withSessionRefresh(send)(get());
    expect(res.status).toBe(200);
    expect(held.getAccessToken).toHaveBeenCalledTimes(1);
    expect(held.getAccessToken).toHaveBeenCalledWith({ rejected: "a0" });
    expect(seen).toEqual(["/api/threads", "/api/threads"]);
    expect(sessionStatus()).toBe("ok");
  });

  it("is the 401 itself when the issuer cannot be reached: no verdict on the session, nobody is asked", async () => {
    held.getAccessToken.mockRejectedValue(new AuthUnavailableError());
    const { send } = api(() => json(401));
    const res = await withSessionRefresh(send)(get());
    expect(res.status).toBe(401);
    expect(sessionStatus()).toBe("ok");
  });

  it("says the session has ended when the issuer refuses the refresh token, holds the request, and sends it when the person is back", async () => {
    // the page has had the session: the banner keeps it
    held.getAccessToken.mockResolvedValue(alice);
    await renewSession();
    held.getAccessToken.mockRejectedValue(new SessionEndedError("refused"));
    const { send, seen } = api(
      () => json(401),
      () => json(),
    );
    const pending = withSessionRefresh(send)(get());
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
    expect(seen).toEqual(["/api/threads"]);
    // the popup has stored a new session and says so on the channel
    held.getAccessToken.mockResolvedValue(alice);
    const stop = watchForSignIn();
    new BroadcastChannel("another-agentic.signed-in").postMessage("signed-in");
    const res = await pending;
    stop();
    expect(res.status).toBe(200);
    expect(seen).toEqual(["/api/threads", "/api/threads"]);
    expect(sessionStatus()).toBe("ok");
  });

  it("never loops on a refused refresh token: the 401s are one question, then every request waits for the person", async () => {
    held.getAccessToken.mockResolvedValue(alice);
    await renewSession();
    held.getAccessToken.mockReset();
    held.getAccessToken.mockRejectedValue(new SessionEndedError("refused"));
    const { send, seen } = api(() => json(401));
    const fetcher = withSessionRefresh(send);
    const away = new AbortController();
    const request = () => new Request("http://app.test/api/threads", { signal: away.signal });
    const pending = [fetcher(request()), fetcher(request()), fetcher(request())];
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
    // a request made while the session is ended is sent once and waits too: nothing asks the issuer again
    pending.push(fetcher(request()));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(seen).toHaveLength(4);
    expect(held.getAccessToken).toHaveBeenCalledTimes(1);
    expect(signInNeed()).toBeNull();
    // the person never comes back: each request gets its own 401, once
    away.abort();
    expect((await Promise.all(pending)).map((r) => r.status)).toEqual([401, 401, 401, 401]);
    expect(seen).toHaveLength(4);
    expect(held.getAccessToken).toHaveBeenCalledTimes(1);
  });

  it("is the app's sign-in screen, saying the session ended, when the page meets a refused sign-in before it had any use of it", async () => {
    held.getAccessToken.mockRejectedValue(new SessionEndedError("refused"));
    const { send, seen } = api(() => json(401));
    const res = await withSessionRefresh(send)(get());
    expect(res.status).toBe(401);
    expect(seen).toEqual(["/api/threads"]);
    expect(signInNeed()).toBe("ended");
    expect(sessionStatus()).toBe("ok");
  });

  it("is the app's sign-in screen when nobody has signed in at all: no banner says that a session ended", async () => {
    held.getAccessToken.mockRejectedValue(new SessionEndedError("none"));
    expect(await renewSession()).toBe("unknown");
    expect(signInNeed()).toBe("none");
    expect(sessionStatus()).toBe("ok");
  });

  it("sends nothing held to a person who signs in as somebody else: the call is rejected and the page is read again", async () => {
    held.getAccessToken.mockResolvedValue(alice);
    await renewSession();
    held.getAccessToken.mockRejectedValue(new SessionEndedError("refused"));
    const { send, seen } = api(
      () => json(401),
      () => json(),
    );
    const pending = withSessionRefresh(send)(get());
    const outcome = pending.catch((e: unknown) => e);
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
    held.getAccessToken.mockResolvedValue(bob);
    const stop = watchForSignIn();
    new BroadcastChannel("another-agentic.signed-in").postMessage("signed-in");
    expect(await outcome).toBeInstanceOf(SessionChangedError);
    stop();
    expect(seen).toEqual(["/api/threads"]);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("is the same person, however the e-mail is written, and the subject when there is none", async () => {
    held.getAccessToken.mockResolvedValue(alice);
    await renewSession();
    held.getAccessToken.mockResolvedValue({
      accessToken: "a2",
      claims: { sub: "s-1", email: "alice@example.test" },
    });
    expect(await renewSession()).toBe("alive");
    expect(reload).not.toHaveBeenCalled();
    resetSessionState();
    held.getAccessToken.mockResolvedValue({ accessToken: "x", claims: { sub: "S-9" } });
    await renewSession();
    held.getAccessToken.mockResolvedValue({ accessToken: "y", claims: { sub: "s-10" } });
    expect(await renewSession()).toBe("changed");
    expect(reload).toHaveBeenCalledTimes(1);
  });
});

describe("keeping the session warm in browser mode", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  it("learns whose session it is once, at the start, and has no timer: an hour of an open page asks nothing more", async () => {
    held.getAccessToken.mockResolvedValue(alice);
    const stop = keepSessionWarm();
    await vi.advanceTimersByTimeAsync(0);
    expect(held.getAccessToken).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(60 * 60_000);
    expect(held.getAccessToken).toHaveBeenCalledTimes(1);
    stop();
  });

  it("asks when the window comes back after a while, not on every focus", async () => {
    held.getAccessToken.mockResolvedValue(alice);
    const stop = keepSessionWarm();
    await vi.advanceTimersByTimeAsync(0);
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(0);
    expect(held.getAccessToken).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(FOCUS_GAP_MS + 1);
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(0);
    expect(held.getAccessToken).toHaveBeenCalledTimes(2);
    stop();
  });

  it("waits for the deployment's kind when it is not known yet, and does nothing in edge mode", async () => {
    setBrowserAuth(undefined);
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("{}", { status: 404 })),
    );
    const stop = keepSessionWarm();
    await vi.advanceTimersByTimeAsync(10);
    expect(held.getAccessToken).not.toHaveBeenCalled();
    stop();
  });
});
