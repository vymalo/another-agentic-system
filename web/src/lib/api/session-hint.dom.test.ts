// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import * as config from "@/lib/auth/config";
import { hadSession, rememberSession, withSessionHint } from "./session-hint";

afterEach(() => {
  rememberSession(false);
  vi.restoreAllMocks();
});

describe("the session hint", () => {
  it("is false until a request has been answered with a success", async () => {
    expect(hadSession()).toBe(false);
    const fetcher = withSessionHint(async () => new Response("{}", { status: 401 }));
    await fetcher(new Request("http://x/api/me"));
    expect(hadSession()).toBe(false);
    vi.spyOn(config, "authReady").mockResolvedValue(null);
    await withSessionHint(async () => new Response("{}", { status: 200 }))(
      new Request("http://x/api/me"),
    );
    await vi.waitFor(() => expect(hadSession()).toBe(true));
  });

  it("leaves nothing in storage in browser mode, where the session is IndexedDB's (ADR 0054)", async () => {
    vi.spyOn(config, "authReady").mockResolvedValue({
      issuer: "https://issuer.example",
      clientId: "web",
      scope: "openid",
    });
    await withSessionHint(async () => new Response("{}", { status: 200 }))(
      new Request("http://x/api/me"),
    );
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(hadSession()).toBe(false);
    expect(window.localStorage.length).toBe(0);
  });

  it("is forgotten on demand", () => {
    rememberSession(true);
    expect(hadSession()).toBe(true);
    rememberSession(false);
    expect(hadSession()).toBe(false);
  });

  it("is a hint, not a requirement: unreadable and unwritable storage is no failure", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(hadSession()).toBe(false);
    expect(() => rememberSession(true)).not.toThrow();
  });
});
