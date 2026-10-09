import type { AddressInfo } from "node:net";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { CLIENT_ID } from "./issuer";
import { createMockServer } from "./server";

/*
 * The two switches that let the desktop app (apps/tauri) run against the mock: the loopback redirect of RFC 8252
 * (`MOCK_LOOPBACK`, as Keycloak matches `http://127.0.0.1/callback` on any port) and the orchestrator's CORS for the
 * webview's origin (`MOCK_CORS_ORIGINS`, as `server.cors` answers it). Off, the mock is the browser deployment it was.
 */

type Server = ReturnType<typeof createMockServer>;
const listen = (s: Server) =>
  new Promise<string>((resolve) =>
    s.listen(0, "127.0.0.1", () =>
      resolve(`http://127.0.0.1:${(s.address() as AddressInfo).port}`),
    ),
  );
const servers: Server[] = [];
const start = async (options: Parameters<typeof createMockServer>[0]) => {
  const server = createMockServer(options);
  servers.push(server);
  return listen(server);
};
afterAll(() => {
  for (const s of servers) s.close();
});

const authorize = (origin: string, redirect: string) => {
  const url = new URL(`${origin}/oidc/auth`);
  url.searchParams.set("client_id", CLIENT_ID);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("redirect_uri", redirect);
  url.searchParams.set("code_challenge", "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
  url.searchParams.set("code_challenge_method", "S256");
  url.searchParams.set("state", "s");
  return fetch(url, { redirect: "manual" });
};

describe("the mock for the desktop app", () => {
  let desktop = "";
  let browser = "";
  beforeAll(async () => {
    desktop = await start({
      browserAuth: {
        origin: "http://127.0.0.1:1",
        publicOrigins: ["tauri://localhost"],
        loopback: true,
      },
      corsOrigins: ["tauri://localhost"],
    });
    browser = await start({
      browserAuth: { origin: "http://127.0.0.1:1", publicOrigins: ["http://app.test"] },
    });
  });

  it("sends the browser back to a loopback redirect on any port, only when asked to", async () => {
    for (const port of [43210, 51234]) {
      const res = await authorize(desktop, `http://127.0.0.1:${port}/callback`);
      expect(res.status).toBe(302);
      expect(res.headers.get("location")).toMatch(
        new RegExp(`^http://127\\.0\\.0\\.1:${port}/callback\\?code=`),
      );
    }
    for (const bad of ["http://localhost:43210/callback", "http://127.0.0.1:43210/other"]) {
      expect((await authorize(desktop, bad)).status).toBe(400);
    }
    expect((await authorize(browser, "http://127.0.0.1:43210/callback")).status).toBe(400);
  });

  it("answers a preflight from a listed origin before identity, and nothing for another", async () => {
    const preflight = (origin: string, base: string) =>
      fetch(`${base}/api/threads`, {
        method: "OPTIONS",
        headers: {
          origin,
          "access-control-request-method": "POST",
          "access-control-request-headers": "authorization, dpop, content-type",
        },
      });
    const ok = await preflight("tauri://localhost", desktop);
    expect(ok.status).toBe(204);
    expect(ok.headers.get("access-control-allow-origin")).toBe("tauri://localhost");
    expect(ok.headers.get("access-control-allow-headers")).toContain("dpop");
    expect(ok.headers.get("access-control-allow-credentials")).toBeNull();
    const other = await preflight("https://evil.example", desktop);
    expect(other.headers.get("access-control-allow-origin")).toBeNull();
    expect(
      (await preflight("tauri://localhost", browser)).headers.get("access-control-allow-origin"),
    ).toBeNull();
    // a request with no token is still refused, and the page may read why
    const refused = await fetch(`${desktop}/api/threads`, {
      headers: { origin: "tauri://localhost" },
    });
    expect(refused.status).toBe(401);
    expect(refused.headers.get("access-control-expose-headers")).toContain("www-authenticate");
  });
});
