import "fake-indexeddb/auto";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { observeDate } from "./clock";
import { authDb } from "./db";
import { ALICE, BOB, openPage, type Page, signInAs } from "./harness";
import { storedKeyPair } from "./keys";
import { completeSignIn, safeReturnTo, startSignIn } from "./sign-in";
import { signOut } from "./sign-out";
import { CLIENT_ID, ISSUER, ORIGIN, thumbprint } from "./test-issuer";
import { AuthUnavailableError, getAccessToken, SessionEndedError, sessionId } from "./tokens";

let page: Page;
beforeEach(() => {
  page = openPage();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const id = sessionId({ issuer: ISSUER, clientId: CLIENT_ID, scope: "" });
const row = () => authDb().session.get(id);
/** The access token has 10 seconds left: due for a refresh. */
const nearExpiry = () => authDb().session.update(id, { expiresAt: Date.now() + 10_000 });

describe("signing in", () => {
  it("sends the browser to the issuer with PKCE S256, a state, the scope and the callback", async () => {
    await startSignIn({ returnTo: "/threads/9" });
    const url = new URL(page.went[0] as string);
    expect(url.origin + url.pathname).toBe(`${ISSUER}/auth`);
    expect(url.searchParams.get("response_type")).toBe("code");
    expect(url.searchParams.get("client_id")).toBe(CLIENT_ID);
    expect(url.searchParams.get("redirect_uri")).toBe(`${ORIGIN}/auth/callback`);
    expect(url.searchParams.get("scope")).toBe("openid email profile offline_access");
    expect(url.searchParams.get("code_challenge_method")).toBe("S256");
    expect(url.searchParams.get("code_challenge")).toMatch(/^[\w-]{43}$/);
    const state = url.searchParams.get("state") as string;
    const pending = await authDb().pending.get(state);
    expect(pending).toMatchObject({ returnTo: "/threads/9", mode: "redirect" });
    // the verifier stays in IndexedDB and is not in the URL
    expect(url.href).not.toContain(pending?.verifier as string);
  });

  it("returns only to a page of this origin", () => {
    expect(safeReturnTo("/threads/1?a=b#c")).toBe("/threads/1?a=b#c");
    for (const bad of [
      "https://evil.test/",
      "//evil.test",
      "/\\evil.test",
      "javascript:1",
      "",
      7,
    ]) {
      expect(safeReturnTo(bad)).toBe("/");
    }
  });

  it("exchanges the code with a DPoP proof, stores a key-bound session and says where to return", async () => {
    const done = await signInAs(page, ALICE, "/threads/9");
    expect(done).toEqual({ returnTo: "/threads/9", mode: "redirect" });
    const stored = await row();
    expect(stored?.claims).toEqual({ sub: ALICE.sub, email: ALICE.email });
    expect(stored?.refreshToken).toMatch(/^refresh-/);
    // the access token is bound to the stored key (the issuer put the thumbprint in `cnf`)
    const key = await storedKeyPair();
    const jwk = await crypto.subtle.exportKey("jwk", (key as CryptoKeyPair).publicKey);
    const token = stored?.accessToken ?? "";
    const payload = JSON.parse(atob(token.split(".")[1] ?? ""));
    expect(payload.cnf.jkt).toBe(await thumbprint(jwk));
    expect(await authDb().pending.count()).toBe(0);
  });

  it("makes a key that cannot be exported", async () => {
    await signInAs(page, ALICE);
    const key = (await storedKeyPair()) as CryptoKeyPair;
    expect(key.privateKey.extractable).toBe(false);
    await expect(crypto.subtle.exportKey("jwk", key.privateKey)).rejects.toThrow();
  });

  it("spends a state once: a replayed callback is refused", async () => {
    await startSignIn({ returnTo: "/" });
    const url = new URL(page.went[0] as string);
    const state = url.searchParams.get("state") as string;
    const code = page.issuer.approve(ALICE, url.searchParams.get("code_challenge") as string);
    const callback = `${ORIGIN}/auth/callback?code=${code}&state=${state}`;
    await completeSignIn(callback);
    await expect(completeSignIn(callback)).rejects.toThrow(/unknown or has expired/);
  });

  it("refuses a callback whose state it never made", async () => {
    await expect(completeSignIn(`${ORIGIN}/auth/callback?code=x&state=nope`)).rejects.toThrow(
      /unknown or has expired/,
    );
  });

  it("forgets a sign-in that was never finished after ten minutes", async () => {
    const now = Date.now();
    await authDb().pending.put({
      state: "old",
      verifier: "v",
      returnTo: "/",
      mode: "redirect",
      createdAt: now - 11 * 60_000,
    });
    await startSignIn({ returnTo: "/" });
    expect(await authDb().pending.get("old")).toBeUndefined();
  });

  it("proves the right time when the local clock is an hour behind the server's", async () => {
    observeDate(new Date(Date.now() + 3_600_000).toUTCString());
    page.issuer.skew = 3600;
    await expect(signInAs(page, ALICE)).resolves.toBeDefined();
  });
});

describe("the access token", () => {
  it("is handed out as it is while it has more than a minute left", async () => {
    await signInAs(page, ALICE);
    const first = await getAccessToken();
    const second = await getAccessToken();
    expect(second.accessToken).toBe(first.accessToken);
    expect(page.issuer.grants()).toBe(0);
  });

  it("is refreshed, with a proof, when less than a minute is left, and the refresh token is rotated", async () => {
    await signInAs(page, ALICE);
    await nearExpiry();
    const before = await row();
    const held = await getAccessToken();
    const after = await row();
    expect(page.issuer.grants()).toBe(1);
    expect(held.accessToken).not.toBe(before?.accessToken);
    expect(after?.refreshToken).toBeDefined();
    expect(after?.refreshToken).not.toBe(before?.refreshToken);
    expect(held.claims.email).toBe(ALICE.email);
  });

  it("is refreshed when the orchestrator refused it, though it has time left", async () => {
    await signInAs(page, ALICE);
    const first = await getAccessToken();
    const next = await getAccessToken({ rejected: first.accessToken });
    expect(page.issuer.grants()).toBe(1);
    expect(next.accessToken).not.toBe(first.accessToken);
  });

  it("is refreshed once when many ask at once: one tab spends the refresh token", async () => {
    await signInAs(page, ALICE);
    await nearExpiry();
    const held = await Promise.all(Array.from({ length: 5 }, () => getAccessToken()));
    expect(page.issuer.grants()).toBe(1);
    expect(new Set(held.map((h) => h.accessToken)).size).toBe(1);
  });

  it("is the one another tab stored: the row is read again inside the lock, and nothing is spent", async () => {
    await signInAs(page, ALICE);
    await nearExpiry();
    const stored = (await row()) as NonNullable<Awaited<ReturnType<typeof row>>>;
    // another tab holds the lock, refreshes, and lets go: what this tab read before is old
    const newer = { ...stored, accessToken: "newer.token", expiresAt: Date.now() + 200_000 };
    vi.stubGlobal("navigator", {
      locks: {
        request: async (_name: string, task: () => Promise<unknown>) => {
          await authDb().session.put(newer);
          return task();
        },
      },
    });
    const held = await getAccessToken();
    expect(held.accessToken).toBe("newer.token");
    expect(page.issuer.grants()).toBe(0);
  });

  it("ends the session when the issuer refuses the refresh token: no more calls, a tombstone stays", async () => {
    await signInAs(page, ALICE);
    await nearExpiry();
    page.issuer.revokeAll();
    await expect(getAccessToken()).rejects.toMatchObject({ reason: "refused" });
    expect(await row()).toMatchObject({ ended: true, accessToken: "", claims: { sub: ALICE.sub } });
    expect((await row())?.refreshToken).toBeUndefined();
    const calls = page.issuer.calls.length;
    await expect(getAccessToken()).rejects.toBeInstanceOf(SessionEndedError);
    expect(page.issuer.calls.length).toBe(calls);
  });

  it("ends the session when a refresh token is used twice (reuse detection)", async () => {
    await signInAs(page, ALICE);
    await nearExpiry();
    const spent = (await row())?.refreshToken as string;
    await getAccessToken();
    // a copy of the row from before the rotation comes back, as a second browser profile would
    await authDb().session.update(id, { refreshToken: spent, expiresAt: 0 });
    await expect(getAccessToken()).rejects.toMatchObject({ reason: "refused" });
  });

  it("does not end the session when the issuer cannot be reached", async () => {
    await signInAs(page, ALICE);
    await nearExpiry();
    page.issuer.down = true;
    await expect(getAccessToken()).rejects.toBeInstanceOf(AuthUnavailableError);
    expect((await row())?.ended).toBeUndefined();
    page.issuer.down = false;
    await expect(getAccessToken()).resolves.toBeDefined();
  });

  it("says nobody is signed in when there is no session", async () => {
    await expect(getAccessToken()).rejects.toMatchObject({ reason: "none" });
  });

  it("is a new person's when somebody else signs in over the session", async () => {
    await signInAs(page, ALICE);
    await signInAs(page, BOB);
    expect((await getAccessToken()).claims.email).toBe(BOB.email);
  });
});

describe("signing out", () => {
  it("revokes the refresh token with a proof, deletes every row and the key, and ends at the issuer", async () => {
    await signInAs(page, ALICE);
    const token = (await row())?.refreshToken as string;
    await signOut();
    expect(page.issuer.revoked).toEqual([token]);
    expect(await authDb().session.count()).toBe(0);
    expect(await authDb().keys.count()).toBe(0);
    expect(await authDb().pending.count()).toBe(0);
    const end = new URL(page.went.at(-1) as string);
    expect(end.origin + end.pathname).toBe(`${ISSUER}/logout`);
    expect(end.searchParams.get("client_id")).toBe(CLIENT_ID);
    expect(end.searchParams.get("post_logout_redirect_uri")).toBe(`${ORIGIN}/`);
  });
});
