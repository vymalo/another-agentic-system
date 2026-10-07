import "fake-indexeddb/auto";
import type { AddressInfo } from "node:net";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { setBrowserAuth } from "../src/lib/auth/config";
import { authDb } from "../src/lib/auth/db";
import { dpopProof } from "../src/lib/auth/dpop";
import { authenticatedFetch, readerFetch, type Send } from "../src/lib/auth/fetch";
import { freshBrowser } from "../src/lib/auth/harness";
import { storedKeyPair } from "../src/lib/auth/keys";
import { navigation } from "../src/lib/auth/navigation";
import { completeSignIn, startSignIn } from "../src/lib/auth/sign-in";
import { signOut } from "../src/lib/auth/sign-out";
import { getAccessToken, sessionId } from "../src/lib/auth/tokens";
import { createMockServer } from "./server";

/*
 * The web's auth module against the mock's issuer and its DPoP check, over real HTTP: what the
 * Playwright specs of browser mode rely on, proved without a browser. The mock plays the issuer
 * (`mock/issuer.ts`) and the orchestrator's `auth-jwt`; the module is the real one.
 */

const APP = "http://app.test";
let origin = "";
let server: ReturnType<typeof createMockServer>;
let edge: ReturnType<typeof createMockServer>;
let edgeOrigin = "";

beforeAll(async () => {
  const listen = (s: ReturnType<typeof createMockServer>, port: number) =>
    new Promise<string>((resolve) =>
      s.listen(port, "127.0.0.1", () =>
        resolve(`http://127.0.0.1:${(s.address() as AddressInfo).port}`),
      ),
    );
  // the issuer's origin is the mock's own: find a free port, then serve there
  const probe = createMockServer();
  const free = Number((await listen(probe, 0)).split(":").pop());
  probe.close();
  edge = createMockServer();
  edgeOrigin = await listen(edge, 0);
  server = createMockServer({
    browserAuth: { origin: `http://127.0.0.1:${free}`, publicOrigins: [APP] },
  });
  origin = await listen(server, free);
});
afterAll(() => {
  server.close();
  edge.close();
});

let sid = "";
let counter = 0;
const cookie = () => ({ cookie: `mock-registry=${sid}` });
const hook = (path: string, method = "POST") =>
  fetch(`${origin}${path}${path.includes("?") ? "&" : "?"}session=${sid}`, { method });
const stats = async () =>
  (await (await fetch(`${origin}/__mock/issuer?session=${sid}`)).json()) as Record<
    string,
    unknown
  > & {
    apiRequests: number;
    refreshGrants: number;
    reuses: number;
    revocations: number;
    endSessions: number;
    publicWithCredentials: number;
    refused: Array<{ error: string; description: string }>;
  };

/** The page's `fetch`, sent to the mock: the proof says the page's origin, the bytes go to the mock. */
const toMock: Send = (request) => {
  const url = new URL(request.url);
  const headers = new Headers(request.headers);
  headers.set("cookie", `mock-registry=${sid}`);
  return fetch(`${origin}${url.pathname}${url.search}`, {
    method: request.method,
    headers,
    ...(request.body ? { body: request.body, duplex: "half" } : {}),
  } as RequestInit);
};

beforeEach(() => {
  freshBrowser();
  sid = `issuer-test-${process.pid}-${++counter}`;
  vi.stubGlobal("window", { location: { origin: APP, pathname: "/", search: "", hash: "" } });
  setBrowserAuth({
    issuer: `${origin}/oidc`,
    clientId: "another-agentic-web",
    scope: "openid email profile offline_access",
  });
  const went: string[] = [];
  vi.spyOn(navigation, "go").mockImplementation((url) => void went.push(url));
  current = { went };
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});
let current: { went: string[] };

/** The whole sign-in against the mock: the redirect, the issuer approving, the callback. */
async function signIn(returnTo = "/threads/1") {
  await startSignIn({ returnTo });
  const authorize = current.went.at(-1) as string;
  const res = await fetch(authorize, { redirect: "manual", headers: cookie() });
  expect(res.status).toBe(302);
  return completeSignIn(res.headers.get("Location") as string);
}

const nearExpiry = () =>
  authDb()
    .session.toCollection()
    .modify({ expiresAt: Date.now() + 5_000 });

describe("the mock as an issuer and as an orchestrator that wants DPoP", () => {
  it("names the issuer at /api/public/auth in browser mode and answers 404 as an edge", async () => {
    const res = await fetch(`${origin}/api/public/auth`);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({
      issuer: `${origin}/oidc`,
      clientId: "another-agentic-web",
      scope: "openid email profile offline_access",
    });
    expect((await fetch(`${edgeOrigin}/api/public/auth`)).status).toBe(404);
  });

  it("signs the person in with PKCE and DPoP, and the API takes a request with the right headers", async () => {
    const done = await signIn("/threads/1");
    expect(done).toEqual({ returnTo: "/threads/1", mode: "redirect" });
    const res = await authenticatedFetch(new Request(`${APP}/api/me`), toMock);
    expect(res.status).toBe(200);
    expect(((await res.json()) as { user: string }).user).toBe("dev@example.com");
    expect((await stats()).apiRequests).toBe(1);
    expect((await stats()).refused).toEqual([]);
  });

  it("refuses the same token sent as Bearer, a replayed proof, another URL's proof, and another key's", async () => {
    await signIn();
    const { accessToken } = await getAccessToken();
    const pair = (await storedKeyPair()) as CryptoKeyPair;
    const send = (headers: Record<string, string>, path = "/api/me") =>
      fetch(`${origin}${path}`, { headers: { ...cookie(), ...headers } });

    const bearer = await send({ Authorization: `Bearer ${accessToken}` });
    expect(bearer.status).toBe(401);
    expect(bearer.headers.get("WWW-Authenticate")).toMatch(/^DPoP error="invalid_token"/);

    const proof = await dpopProof(pair, { method: "GET", url: `${APP}/api/me`, accessToken });
    const good = { Authorization: `DPoP ${accessToken}`, DPoP: proof };
    expect((await send(good)).status).toBe(200);
    const replay = await send(good);
    expect(replay.status).toBe(401);
    expect(replay.headers.get("WWW-Authenticate")).toMatch(/error="invalid_dpop_proof"/);

    const otherUrl = await dpopProof(pair, {
      method: "GET",
      url: `${APP}/api/threads`,
      accessToken,
    });
    expect((await send({ Authorization: `DPoP ${accessToken}`, DPoP: otherUrl })).status).toBe(401);
    const otherMethod = await dpopProof(pair, {
      method: "POST",
      url: `${APP}/api/me`,
      accessToken,
    });
    expect((await send({ Authorization: `DPoP ${accessToken}`, DPoP: otherMethod })).status).toBe(
      401,
    );
    const noAth = await dpopProof(pair, { method: "GET", url: `${APP}/api/me` });
    expect((await send({ Authorization: `DPoP ${accessToken}`, DPoP: noAth })).status).toBe(401);

    const stranger = await crypto.subtle.generateKey(
      { name: "ECDSA", namedCurve: "P-256" },
      false,
      ["sign", "verify"],
    );
    const foreign = await dpopProof(stranger, { method: "GET", url: `${APP}/api/me`, accessToken });
    const stolen = await send({ Authorization: `DPoP ${accessToken}`, DPoP: foreign });
    expect(stolen.status).toBe(401);
    expect((await stats()).refused.at(-1)?.description).toMatch(/cnf\.jkt/);

    expect((await send({})).status).toBe(401);
  });

  it("rotates the refresh token, and a refresh token used twice ends the session", async () => {
    await signIn();
    const stored = await authDb().session.get(
      sessionId({ issuer: `${origin}/oidc`, clientId: "another-agentic-web", scope: "" }),
    );
    await nearExpiry();
    await getAccessToken();
    expect((await stats()).refreshGrants).toBe(1);
    // the first token comes back from a copy of the profile
    await authDb()
      .session.toCollection()
      .modify({ refreshToken: stored?.refreshToken, expiresAt: 0 });
    await expect(getAccessToken()).rejects.toMatchObject({ reason: "refused" });
    expect((await stats()).reuses).toBe(1);
  });

  it("refuses a refresh token an administrator revoked", async () => {
    await signIn();
    await hook("/__mock/issuer-revoke");
    await nearExpiry();
    await expect(getAccessToken()).rejects.toMatchObject({ reason: "refused" });
  });

  it("is signed in as the person a test chose", async () => {
    await hook("/__mock/issuer-config?loginAs=admin");
    await signIn();
    const res = await authenticatedFetch(new Request(`${APP}/api/me`), toMock);
    expect(((await res.json()) as { user: string }).user).toBe("admin@example.com");
  });

  it("counts a refresh once however many ask at once, even when the issuer is slow", async () => {
    await hook("/__mock/issuer-config?refreshDelay=150");
    await signIn();
    await nearExpiry();
    await Promise.all(Array.from({ length: 4 }, () => getAccessToken()));
    expect((await stats()).refreshGrants).toBe(1);
    expect((await stats()).reuses).toBe(0);
  });

  it("revokes at sign-out, with a proof, and ends the issuer's session", async () => {
    await signIn();
    await signOut();
    const s = await stats();
    expect(s.revocations).toBe(1);
    const logout = new URL(current.went.at(-1) as string);
    expect(logout.pathname).toBe("/oidc/logout");
    const res = await fetch(logout, { redirect: "manual", headers: cookie() });
    expect(res.headers.get("Location")).toBe(`${APP}/`);
    expect((await stats()).endSessions).toBe(1);
  });

  it("sees no credentials on a public route from a reader of a public link", async () => {
    const res = await readerFetch(
      new Request(`${APP}/api/public/shared/${"a".repeat(43)}`),
      toMock,
    );
    expect(res.status).toBe(404);
    expect((await stats()).publicWithCredentials).toBe(0);
    // and one that carried them would have been counted
    await fetch(`${origin}/api/public/shared/x`, {
      headers: { ...cookie(), Authorization: "DPoP x" },
    });
    expect((await stats()).publicWithCredentials).toBe(1);
  });
});
