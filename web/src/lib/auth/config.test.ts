import { afterEach, describe, expect, it, vi } from "vitest";
import { serverNowSeconds } from "./clock";
import { authReady, browserAuth, setBrowserAuth } from "./config";

afterEach(() => {
  vi.unstubAllGlobals();
  setBrowserAuth(null);
});

const stub = (answer: () => Response | Promise<Response>) => {
  const fetch = vi.fn(async () => answer());
  vi.stubGlobal("fetch", fetch);
  return fetch;
};
const ok = (body: unknown, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), { status: 200, headers });

describe("which kind of deployment this is", () => {
  it("is browser mode when the orchestrator names an issuer, and asks once however many ask", async () => {
    setBrowserAuth(undefined);
    const fetch = stub(() =>
      ok({ issuer: "https://id.example/realms/x", clientId: "web", scope: "openid" }),
    );
    expect(browserAuth()).toBeUndefined();
    const [a, b] = await Promise.all([authReady(), authReady()]);
    expect(a).toEqual({ issuer: "https://id.example/realms/x", clientId: "web", scope: "openid" });
    expect(b).toBe(a);
    expect(browserAuth()).toBe(a);
    expect(fetch).toHaveBeenCalledTimes(1);
    const [path, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe("/api/public/auth");
    // a public route: no cookie, no token
    expect(init.credentials).toBe("omit");
    await authReady();
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("is edge mode on a 404: the web of today", async () => {
    setBrowserAuth(undefined);
    stub(() => new Response("{}", { status: 404 }));
    expect(await authReady()).toBeNull();
  });

  it("is edge mode on a network error", async () => {
    setBrowserAuth(undefined);
    stub(() => Promise.reject(new TypeError("offline")));
    expect(await authReady()).toBeNull();
  });

  it("is edge mode when the answer cannot be trusted: a plain http issuer that is not on loopback, or a half answer", async () => {
    for (const body of [
      { issuer: "http://id.example", clientId: "web", scope: "openid" },
      { issuer: "https://id.example", scope: "openid" },
      { issuer: "javascript:1", clientId: "web", scope: "openid" },
      "text",
    ]) {
      setBrowserAuth(undefined);
      stub(() => ok(body));
      expect(await authReady()).toBeNull();
    }
  });

  it("accepts plain http on the loopback address (a local run)", async () => {
    setBrowserAuth(undefined);
    stub(() => ok({ issuer: "http://127.0.0.1:4010/oidc", clientId: "web", scope: "openid" }));
    expect(await authReady()).not.toBeNull();
  });

  it("learns the server's clock from the answer's Date header", async () => {
    setBrowserAuth(undefined);
    const ahead = new Date(Date.now() + 90_000).toUTCString();
    stub(() =>
      ok({ issuer: "https://id.example", clientId: "web", scope: "openid" }, { Date: ahead }),
    );
    await authReady();
    expect(serverNowSeconds() - Math.floor(Date.now() / 1000)).toBeGreaterThanOrEqual(89);
  });
});
