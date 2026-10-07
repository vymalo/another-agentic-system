import "fake-indexeddb/auto";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { authDb } from "./db";
import { authenticatedFetch, lastRejectedToken, readerFetch } from "./fetch";
import { ALICE, openPage, type Page, signInAs } from "./harness";
import { verifyProof } from "./test-issuer";
import { getAccessToken } from "./tokens";

let page: Page;
beforeEach(() => {
  page = openPage();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** The claims of a proof, read without checking: the orchestrator's check is `verifyProof`. */
const claimsOfProof = (proof: string): Record<string, unknown> =>
  JSON.parse(atob((proof.split(".")[1] ?? "").replace(/-/g, "+").replace(/_/g, "/")));

const URL_OF = (path: string) => `https://app.test${path}`;
const now = () => Math.floor(Date.now() / 1000);

/** An orchestrator that checks DPoP the way the real one does, and answers with `status`. */
function orchestrator(
  answer: (call: number, request: Request) => Response | undefined = () => undefined,
) {
  const seen: { token: string; proof: string; url: string; method: string; body: string }[] = [];
  const send = async (request: Request): Promise<Response> => {
    const authorization = request.headers.get("Authorization") ?? "";
    const proof = request.headers.get("DPoP") ?? "";
    const token = authorization.replace(/^DPoP /, "");
    seen.push({
      token,
      proof,
      url: request.url,
      method: request.method,
      body: request.body ? await request.clone().text() : "",
    });
    const custom = answer(seen.length, request);
    if (custom) return custom;
    if (!authorization.startsWith("DPoP ")) return new Response(null, { status: 401 });
    await verifyProof(
      proof,
      { method: request.method, url: request.url, accessToken: token },
      now(),
    );
    return new Response("{}", { status: 200 });
  };
  return { seen, send };
}

describe("a request to the orchestrator in browser mode", () => {
  it("carries Authorization: DPoP and a proof of its method, its URL without the query, and the token's hash", async () => {
    await signInAs(page, ALICE);
    const api = orchestrator();
    const res = await authenticatedFetch(new Request(URL_OF("/api/threads?limit=5#x")), api.send);
    expect(res.status).toBe(200);
    expect(api.seen).toHaveLength(1);
    const claims = claimsOfProof(api.seen[0]?.proof ?? "");
    expect(claims.htu).toBe(URL_OF("/api/threads"));
    expect(claims.htm).toBe("GET");
  });

  it("is signed anew every time: a proof is never sent twice", async () => {
    await signInAs(page, ALICE);
    const api = orchestrator();
    await authenticatedFetch(new Request(URL_OF("/api/threads")), api.send);
    await authenticatedFetch(new Request(URL_OF("/api/threads")), api.send);
    expect(new Set(api.seen.map((s) => s.proof)).size).toBe(2);
  });

  it("covers the AG-UI routes too, and keeps a POST's body", async () => {
    await signInAs(page, ALICE);
    const api = orchestrator();
    const res = await authenticatedFetch(
      new Request(URL_OF("/agui/agents/chat"), { method: "POST", body: '{"a":1}' }),
      api.send,
    );
    expect(res.status).toBe(200);
    expect(api.seen[0]).toMatchObject({ method: "POST", body: '{"a":1}' });
  });

  it("is refreshed before it is sent when the token has less than a minute left", async () => {
    await signInAs(page, ALICE);
    await authDb()
      .session.toCollection()
      .modify({ expiresAt: Date.now() + 5_000 });
    const before = (await getAccessToken({ rejected: "" })).accessToken;
    expect(page.issuer.grants()).toBe(1);
    await authDb()
      .session.toCollection()
      .modify({ expiresAt: Date.now() + 5_000 });
    const api = orchestrator();
    await authenticatedFetch(new Request(URL_OF("/api/me")), api.send);
    expect(page.issuer.grants()).toBe(2);
    expect(api.seen[0]?.token).not.toBe(before);
  });

  it("is sent again once, with a new proof and the whole body, when the proof was refused", async () => {
    await signInAs(page, ALICE);
    const api = orchestrator((call) =>
      call === 1
        ? new Response(null, {
            status: 401,
            headers: { "WWW-Authenticate": 'DPoP error="invalid_dpop_proof"' },
          })
        : undefined,
    );
    const res = await authenticatedFetch(
      new Request(URL_OF("/agui/agents/chat"), { method: "POST", body: "payload" }),
      api.send,
    );
    expect(res.status).toBe(200);
    expect(api.seen).toHaveLength(2);
    expect(api.seen[0]?.proof).not.toBe(api.seen[1]?.proof);
    expect(api.seen[1]?.body).toBe("payload");
    expect(page.issuer.grants()).toBe(0);
  });

  it("hands a 401 invalid_token back, and remembers which token was refused", async () => {
    await signInAs(page, ALICE);
    const api = orchestrator(
      () =>
        new Response(null, {
          status: 401,
          headers: { "WWW-Authenticate": 'DPoP error="invalid_token"' },
        }),
    );
    const res = await authenticatedFetch(new Request(URL_OF("/api/me")), api.send);
    expect(res.status).toBe(401);
    expect(lastRejectedToken()).toBe(api.seen[0]?.token);
    // the next token is a new one, though the old one has time left
    const next = await getAccessToken({ rejected: lastRejectedToken() as string });
    expect(next.accessToken).not.toBe(api.seen[0]?.token);
  });

  it("is a 401 made here, with nothing sent, when the sign-in has ended", async () => {
    await signInAs(page, ALICE);
    await authDb().session.toCollection().modify({ expiresAt: 0 });
    page.issuer.revokeAll();
    const api = orchestrator();
    const res = await authenticatedFetch(new Request(URL_OF("/api/me")), api.send);
    expect(res.status).toBe(401);
    expect(res.headers.get("WWW-Authenticate")).toContain("invalid_token");
    expect(api.seen).toHaveLength(0);
  });

  it("takes the browser to the issuer, and waits, when nobody has signed in", async () => {
    const api = orchestrator();
    const outcome = await Promise.race([
      authenticatedFetch(new Request(URL_OF("/api/me")), api.send).then(() => "answered"),
      new Promise((resolve) => setTimeout(() => resolve("waiting"), 100)),
    ]);
    expect(outcome).toBe("waiting");
    expect(page.went).toHaveLength(1);
    expect(page.went[0]).toContain("https://issuer.test/realms/demo/auth?");
    expect(api.seen).toHaveLength(0);
  });
});

describe("the public routes and the readers of a share link", () => {
  it("never carry a token and never open IndexedDB", async () => {
    const api = orchestrator(() => new Response("{}", { status: 200 }));
    for (const path of ["/api/public/shared/abc", "/agui/public/shared/abc/connect"]) {
      await authenticatedFetch(new Request(URL_OF(path)), api.send);
      await readerFetch(new Request(URL_OF(path)), api.send);
    }
    expect(api.seen.map((s) => s.token)).toEqual(["", "", "", ""]);
    expect(api.seen.map((s) => s.proof)).toEqual(["", "", "", ""]);
    expect(await indexedDB.databases()).toEqual([]);
  });

  it("answer the signed-in route 401 with nothing sent, and nothing created, for a reader who never signed in", async () => {
    const api = orchestrator();
    const res = await readerFetch(new Request(URL_OF("/api/shared/abc")), api.send);
    expect(res.status).toBe(401);
    expect(api.seen).toHaveLength(0);
    expect(await indexedDB.databases()).toEqual([]);
    expect(page.went).toEqual([]);
  });

  it("use the stored session of a reader who is signed in", async () => {
    await signInAs(page, ALICE);
    const api = orchestrator();
    const res = await readerFetch(new Request(URL_OF("/api/shared/abc")), api.send);
    expect(res.status).toBe(200);
    expect(api.seen[0]?.token).not.toBe("");
  });
});

describe("a deployment with no sign-in of its own (edge mode)", () => {
  it("is sent untouched, with no header added and nothing opened", async () => {
    const { setBrowserAuth } = await import("./config");
    setBrowserAuth(null);
    const api = orchestrator(() => new Response("{}", { status: 200 }));
    await authenticatedFetch(new Request(URL_OF("/api/me")), api.send);
    expect(api.seen[0]).toMatchObject({ token: "", proof: "" });
    expect(await indexedDB.databases()).toEqual([]);
  });
});
