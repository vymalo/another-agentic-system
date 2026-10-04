// @vitest-environment jsdom
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { resetSessionState, sessionStatus } from "./session-refresh";

/*
 * Which clients meet the session's refresh (session-refresh.ts) and which never do: `api` (the app)
 * does, `readerApi` (a shared page's reader, ADR 0040) must never go near a sign-in.
 */

const RealRequest = globalThis.Request;
let api: typeof import("./client").api;
let readerApi: typeof import("./client").readerApi;
let asked: string[];
beforeAll(async () => {
  // a browser resolves `/api/me` against the page; the test's Request does it against a made-up origin.
  // The clients keep the `Request` they find when they are made, so it is in place before they are.
  vi.stubGlobal(
    "Request",
    class extends RealRequest {
      constructor(input: RequestInfo | URL, init?: RequestInit) {
        super(
          typeof input === "string" && input.startsWith("/") ? `http://app.test${input}` : input,
          init,
        );
      }
    },
  );
  // and so does `fetch`: the reader's client is plain openapi-fetch, which keeps the one it finds
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(input instanceof Request ? input.url : String(input), "http://app.test");
      asked.push(url.pathname);
      // the session is gone everywhere: the app's calls, and the edge's own answer
      return new Response("{}", { status: 401, headers: { "content-type": "application/json" } });
    }),
  );
  ({ api, readerApi } = await import("./client"));
});
afterAll(() => {
  vi.unstubAllGlobals();
});
beforeEach(() => {
  resetSessionState();
  asked = [];
  vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
});
afterEach(() => {
  vi.unstubAllEnvs();
});

describe("the clients and a 401", () => {
  it("the app's client asks the edge about the session, and the page is told it has ended", async () => {
    void api.GET("/api/me");
    await vi.waitFor(() => expect(sessionStatus()).toBe("ended"));
    expect(asked).toEqual(["/api/me", "/oauth2/userinfo"]);
  });

  it("a shared page's reader never does: a 401 is its answer, and nothing is asked of the edge", async () => {
    const { response } = await readerApi.GET("/api/shared/{token}", {
      params: { path: { token: "t".repeat(43) } },
    });
    expect(response.status).toBe(401);
    expect(asked).toEqual([`/api/shared/${"t".repeat(43)}`]);
    expect(sessionStatus()).toBe("ok");
  });
});
