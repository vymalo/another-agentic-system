import "fake-indexeddb/auto";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { authDb } from "./db";
import { ALICE, openPage, type Page } from "./harness";
import { requireSignIn, resetSignInNeed, signInNeed } from "./sign-in-need";
import { CLIENT_ID, ISSUER } from "./test-issuer";
import { getAccessToken, sessionId } from "./tokens";

/*
 * The desktop app's sign-in (ADR 0047, decision 3): the page makes PKCE and the state as in a browser, the app opens the
 * person's browser at the authorization URL with a loopback redirect (RFC 8252) and hands back where the issuer sent it;
 * the page exchanges the code with its own proof and loads itself again, never the issuer's page. The app's side
 * (`apps/tauri`) is stood in for here.
 */
const app = vi.hoisted(() => ({
  opened: [] as string[],
  approve: undefined as undefined | ((authorize: URL) => string),
}));
vi.mock("./desktop", () => ({
  isLoopback: () => true,
  loopbackRedirect: async () => "http://127.0.0.1:43210/callback",
  loopbackAuthorize: async (url: string) => {
    app.opened.push(url);
    return (
      app.approve?.(new URL(url)) ?? "http://127.0.0.1:43210/callback?error=access_denied&state=x"
    );
  },
  openInBrowser: async (url: string) => {
    app.opened.push(url);
  },
}));

import { startSignIn } from "./sign-in";
import { signOut } from "./sign-out";

let page: Page;
beforeEach(() => {
  page = openPage();
  app.opened = [];
  resetSignInNeed();
  app.approve = (authorize) => {
    const code = page.issuer.approve(ALICE, authorize.searchParams.get("code_challenge") as string);
    return `http://127.0.0.1:43210/callback?code=${code}&state=${authorize.searchParams.get("state")}`;
  };
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("signing in in the desktop app", () => {
  it("opens the person's browser with a loopback redirect, exchanges the code with a proof, and loads the page again", async () => {
    requireSignIn("none");
    const told: unknown[] = [];
    const channel = new BroadcastChannel("another-agentic.signed-in");
    channel.onmessage = (e) => told.push(e.data);
    await startSignIn({ returnTo: "/threads/9" });
    const authorize = new URL(app.opened[0] as string);
    expect(authorize.searchParams.get("redirect_uri")).toBe("http://127.0.0.1:43210/callback");
    expect(authorize.searchParams.get("code_challenge_method")).toBe("S256");
    expect(authorize.searchParams.get("client_id")).toBe(CLIENT_ID);
    // the code came back to this page, which loads again where it was, as a browser's does after its callback
    expect(page.went).toEqual(["/threads/9"]);
    await vi.waitFor(() => expect(told).toEqual(["signed-in"]));
    channel.close();
    const row = await authDb().session.get(
      sessionId({ issuer: ISSUER, clientId: CLIENT_ID, scope: "" }),
    );
    expect(row?.claims.email).toBe(ALICE.email);
    expect((await getAccessToken()).accessToken).toBe(row?.accessToken);
    expect(await authDb().pending.count()).toBe(0);
  });

  it("keeps the page for the banner's sign-in, whose held requests go on", async () => {
    await startSignIn({ returnTo: "/threads/9", stay: true });
    expect(page.went).toEqual([]);
    expect(await authDb().session.count()).toBe(1);
  });

  it("stores nothing, and says so, when the issuer refused (error=access_denied with the right state)", async () => {
    app.approve = (authorize) =>
      `http://127.0.0.1:43210/callback?error=access_denied&state=${authorize.searchParams.get("state")}`;
    requireSignIn("none");
    const refused = await startSignIn().then(
      () => null,
      (e: unknown) => e,
    );
    expect(JSON.stringify(refused, Object.getOwnPropertyNames(refused ?? {}))).toContain(
      "access_denied",
    );
    expect(page.issuer.calls.some((c) => c.grant === "authorization_code")).toBe(false);
    expect(await authDb().session.count()).toBe(0);
    expect(signInNeed()).toBe("none");
  });

  it("exchanges nothing for a callback whose state this page did not make (another local process)", async () => {
    app.approve = (authorize) => {
      const code = page.issuer.approve(
        ALICE,
        authorize.searchParams.get("code_challenge") as string,
      );
      return `http://127.0.0.1:43210/callback?code=${code}&state=not-ours`;
    };
    requireSignIn("none");
    await expect(startSignIn()).rejects.toThrow();
    expect(page.issuer.calls.some((c) => c.grant === "authorization_code")).toBe(false);
    expect(await authDb().session.count()).toBe(0);
    expect(signInNeed()).toBe("none");
  });

  it("signs out in the person's browser, never in the app, and comes back to the start page", async () => {
    await startSignIn({ stay: true });
    app.opened = [];
    await signOut();
    expect(page.issuer.revoked.length).toBe(1);
    const end = new URL(app.opened[0] as string);
    expect(end.origin + end.pathname).toBe(`${ISSUER}/logout`);
    expect(end.searchParams.get("client_id")).toBe(CLIENT_ID);
    // no page of the app is registered at the issuer to come back to
    expect(end.searchParams.has("post_logout_redirect_uri")).toBe(false);
    expect(page.went).toEqual(["/"]);
    expect(await authDb().session.count()).toBe(0);
  });
});
