import "fake-indexeddb/auto";
import * as oauth from "oauth4webapi";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseRuntimeConfig } from "@/lib/runtime-config";

const app = vi.hoisted(() => ({
  open: vi.fn<(url: string) => Promise<void>>(),
  current: vi.fn<() => Promise<string[] | null>>(),
  onUrl: undefined as undefined | ((urls: string[]) => void),
}));

vi.mock("@tauri-apps/plugin-deep-link", () => ({
  getCurrent: app.current,
  onOpenUrl: async (handler: (urls: string[]) => void) => {
    app.onUrl = handler;
    return () => {};
  },
}));

vi.mock("./desktop", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./desktop")>()),
  isClaimed: () => true,
  isLoopback: () => false,
  openInBrowser: app.open,
}));

import { setRuntimeConfig } from "@/lib/runtime-config";
import { authDb } from "./db";
import { ALICE, openPage, type Page } from "./harness";
import { cancelClaimed, isClaimedCallback, startSignIn } from "./sign-in";

describe("the mobile claimed callback", () => {
  let page: Page;

  beforeEach(() => {
    page = openPage();
    app.open.mockReset();
    app.current.mockReset().mockResolvedValue(null);
    setRuntimeConfig({ signIn: "claimed" });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("accepts only the registered HTTPS callback, with its authorization response", () => {
    expect(
      isClaimedCallback(
        "https://agentic.servers.segning.pro/mobile/auth/callback?code=one&state=two",
      ),
    ).toBe(true);
  });

  it("does not let a runtime file choose another callback receiver", () => {
    for (const url of [
      "http://agentic.servers.segning.pro/mobile/auth/callback?code=one&state=two",
      "https://agentic.servers.segning.pro/mobile/auth/callback/other?code=one&state=two",
      "https://agentic.servers.segning.pro/auth/callback?code=one&state=two",
      "https://elsewhere.example/mobile/auth/callback?code=one&state=two",
      "https://agentic.servers.segning.pro/mobile/auth/callback.evil?code=one&state=two",
      "https://agentic.servers.segning.pro/mobile/auth%2Fcallback?code=one&state=two",
      "https://agentic.servers.segning.pro/mobile/auth/callback#code=one&state=two",
    ]) {
      expect(isClaimedCallback(url)).toBe(false);
    }
    expect(
      parseRuntimeConfig({
        signIn: "claimed",
        redirectUri: "https://attacker.example/callback",
      }),
    ).toEqual({ signIn: "claimed" });
  });

  it("forgets the pending state when the native browser cannot open", async () => {
    app.open.mockRejectedValueOnce(new Error("cannot open"));

    await expect(startSignIn()).rejects.toThrow("cannot open");
    expect(await authDb().pending.count()).toBe(0);
  });

  it("lets the person cancel an abandoned native-browser sign-in", async () => {
    app.open.mockResolvedValueOnce();
    const signingIn = startSignIn();
    await vi.waitFor(() => expect(app.open).toHaveBeenCalledOnce());
    await cancelClaimed();
    await expect(signingIn).rejects.toThrow("cancelled");
    expect(await authDb().pending.count()).toBe(0);
  });

  it("opens the issuer in the native browser and exchanges only the claimed deep-link callback", async () => {
    let authorization: URL | undefined;
    app.open.mockImplementation(async (url) => {
      authorization = new URL(url);
    });

    const signingIn = startSignIn({ returnTo: "/threads/9" });
    await vi.waitFor(() => expect(authorization).toBeDefined());
    const url = authorization as URL;
    expect(url.searchParams.get("redirect_uri")).toBe(
      "https://agentic.servers.segning.pro/mobile/auth/callback",
    );
    expect(url.searchParams.get("code_challenge_method")).toBe("S256");

    app.onUrl?.(["https://elsewhere.example/mobile/auth/callback?code=not-ours&state=nope"]);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(page.issuer.calls.some((call) => call.grant === "authorization_code")).toBe(false);

    const code = page.issuer.approve(ALICE, url.searchParams.get("code_challenge") as string);
    app.onUrl?.([
      `https://agentic.servers.segning.pro/mobile/auth/callback?code=${code}&state=${url.searchParams.get("state")}`,
    ]);
    await signingIn;
    expect(page.went).toEqual(["/threads/9"]);
    expect(await authDb().pending.count()).toBe(0);
  });
});

describe("a claimed callback that cold-launches the mobile app", () => {
  let page: Page;
  let db: typeof import("./db");
  let signIn: typeof import("./sign-in");

  beforeEach(async () => {
    // A new webview has no in-memory waiter or listener, but still has the pending sign-in.
    vi.resetModules();
    const harness = await import("./harness");
    page = harness.openPage();
    db = await import("./db");
    signIn = await import("./sign-in");
    const runtime = await import("@/lib/runtime-config");
    runtime.setRuntimeConfig({ signIn: "claimed" });
    app.current.mockReset().mockResolvedValue(null);
    app.open.mockReset();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  async function pendingCallback(createdAt = Date.now()) {
    const state = oauth.generateRandomState();
    const verifier = oauth.generateRandomCodeVerifier();
    const challenge = await oauth.calculatePKCECodeChallenge(verifier);
    const code = page.issuer.approve(ALICE, challenge);
    await db.authDb().pending.put({
      state,
      verifier,
      returnTo: "/threads/9",
      mode: "claimed",
      redirectUri: signIn.CLAIMED_CALLBACK_URI,
      createdAt,
    });
    return { state, url: `${signIn.CLAIMED_CALLBACK_URI}?code=${code}&state=${state}` };
  }

  it("exchanges getCurrent's callback using persisted PKCE and returns to the saved page", async () => {
    const callback = await pendingCallback();
    app.current.mockResolvedValue([callback.url]);

    await signIn.startClaimedCallbackListener();

    expect(page.went).toEqual(["/threads/9"]);
    expect((await db.authDb().session.toArray()).map((row) => row.claims)).toEqual([ALICE]);
    expect(await db.authDb().pending.count()).toBe(0);
    expect(app.open).not.toHaveBeenCalled();
  });

  it("does not establish a session when the issuer refuses the cold-launch callback", async () => {
    const callback = await pendingCallback();
    app.current.mockResolvedValue([
      `${signIn.CLAIMED_CALLBACK_URI}?error=access_denied&state=${callback.state}`,
    ]);

    await signIn.startClaimedCallbackListener();

    expect(page.went).toEqual([]);
    expect(await db.authDb().session.count()).toBe(0);
    expect(await db.authDb().pending.count()).toBe(0);
    expect(page.issuer.calls.some((call) => call.grant === "authorization_code")).toBe(false);
  });

  it("spends expired persisted state without exchanging its authorization code", async () => {
    const callback = await pendingCallback(Date.now() - 10 * 60_000 - 1);
    app.current.mockResolvedValue([callback.url]);

    await signIn.startClaimedCallbackListener();

    expect(page.went).toEqual([]);
    expect(await db.authDb().session.count()).toBe(0);
    expect(await db.authDb().pending.count()).toBe(0);
    expect(page.issuer.calls.some((call) => call.grant === "authorization_code")).toBe(false);
  });

  it("rejects a mismatched state without spending the real pending sign-in", async () => {
    const callback = await pendingCallback();
    const wrongState = new URL(callback.url);
    wrongState.searchParams.set("state", "not-the-persisted-state");
    app.current.mockResolvedValue([wrongState.href]);

    await signIn.startClaimedCallbackListener();

    expect(page.went).toEqual([]);
    expect(await db.authDb().session.count()).toBe(0);
    expect(await db.authDb().pending.get(callback.state)).toBeDefined();
    expect(page.issuer.calls.some((call) => call.grant === "authorization_code")).toBe(false);
  });

  it("ignores an unclaimed receiver even when it carries valid persisted state", async () => {
    const callback = await pendingCallback();
    app.current.mockResolvedValue([
      callback.url.replace("agentic.servers.segning.pro", "attacker.example"),
    ]);

    await signIn.startClaimedCallbackListener();

    expect(page.went).toEqual([]);
    expect(await db.authDb().session.count()).toBe(0);
    expect(await db.authDb().pending.get(callback.state)).toBeDefined();
    expect(page.issuer.calls).toEqual([]);
  });
});
