// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBrowserAuth } from "@/lib/auth/config";
import { navigation } from "@/lib/auth/navigation";
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
