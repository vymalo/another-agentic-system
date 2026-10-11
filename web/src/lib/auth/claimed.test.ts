import "fake-indexeddb/auto";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseRuntimeConfig } from "@/lib/runtime-config";

const app = vi.hoisted(() => ({
  open: vi.fn<(url: string) => Promise<void>>(),
  onUrl: undefined as undefined | ((urls: string[]) => void),
}));

vi.mock("@tauri-apps/plugin-deep-link", () => ({
  getCurrent: async () => null,
  onOpenUrl: async (handler: (urls: string[]) => void) => {
    app.onUrl = handler;
    return () => {};
  },
}));

vi.mock("./desktop", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./desktop")>()),
  openInBrowser: app.open,
}));

import { setRuntimeConfig } from "@/lib/runtime-config";
import { authDb } from "./db";
import { ALICE, openPage, type Page } from "./harness";
import { isClaimedCallback, startSignIn } from "./sign-in";

describe("the mobile claimed callback", () => {
  let page: Page;

  beforeEach(() => {
    page = openPage();
    app.open.mockReset();
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
