// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBrowserAuth } from "@/lib/auth/config";
import * as signIn from "@/lib/auth/sign-in";
import {
  navigation,
  openSignIn,
  REDIRECT_PAUSE_MS,
  redirectToSignIn,
  refreshPath,
  resetRedirectPause,
  SIGNED_IN_PATH,
  signInPath,
  signInUrl,
} from "./session";

let go: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  resetRedirectPause();
  window.history.pushState({}, "", "/threads/abc?tab=sources#m-3");
  go = vi.spyOn(navigation, "go").mockImplementation(() => {});
});
afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

describe("the sign-in path", () => {
  it("is none unless the deployment sets one", () => {
    expect(signInPath()).toBeNull();
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "  ");
    expect(signInPath()).toBeNull();
  });

  it("is a path of this origin: a URL, or a path with a host in front, is none", () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/sign_in");
    expect(signInPath()).toBe("/oauth2/sign_in");
    for (const bad of ["https://idp.example/login", "//idp.example/login", "oauth2/sign_in"]) {
      vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", bad);
      expect(signInPath()).toBeNull();
    }
  });

  it("carries the page to come back to in rd, escaped, after a query the path may have", () => {
    expect(signInUrl("/oauth2/sign_in", "/threads/a?b=c&d=e#m-1")).toBe(
      "/oauth2/sign_in?rd=%2Fthreads%2Fa%3Fb%3Dc%26d%3De%23m-1",
    );
    expect(signInUrl("/oauth2/sign_in?x=1", "/")).toBe("/oauth2/sign_in?x=1&rd=%2F");
  });
});

describe("redirectToSignIn", () => {
  it("does nothing, and says so, when no path is configured", () => {
    expect(redirectToSignIn()).toBe(false);
    expect(go).not.toHaveBeenCalled();
  });

  it("goes to the sign-in with the current page, once per pause", () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/sign_in");
    expect(redirectToSignIn(1_000)).toBe(true);
    expect(go).toHaveBeenCalledWith("/oauth2/sign_in?rd=%2Fthreads%2Fabc%3Ftab%3Dsources%23m-3");
    // a second 401 of the same page load, or of the page it comes back to, is not a second redirect
    expect(redirectToSignIn(1_000 + REDIRECT_PAUSE_MS - 1)).toBe(false);
    expect(go).toHaveBeenCalledTimes(1);
    // a session that expires later in the same tab is a new sign-in
    expect(redirectToSignIn(1_000 + REDIRECT_PAUSE_MS)).toBe(true);
    expect(go).toHaveBeenCalledTimes(2);
  });
});

describe("the refresh path", () => {
  it("is the edge's userinfo, beside the sign-in", () => {
    expect(refreshPath()).toBeNull();
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    expect(refreshPath()).toBe("/oauth2/userinfo");
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/sign_in");
    expect(refreshPath()).toBe("/oauth2/userinfo");
    // a prefix the deployment moved, and a sign-in with a query of its own
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/auth/edge/start?x=1");
    expect(refreshPath()).toBe("/auth/edge/userinfo");
  });

  it("is none where there is no sign-in", () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "https://idp.example/start");
    expect(refreshPath()).toBeNull();
  });
});

describe("openSignIn", () => {
  const fakePopup = () => {
    const popup = { opener: {} as unknown, location: { href: "" } };
    const open = vi.spyOn(window, "open").mockReturnValue(popup as unknown as Window);
    return { popup, open };
  };

  it("does nothing, and says so, where no sign-in is built in", () => {
    const { open } = fakePopup();
    expect(openSignIn()).toBe("none");
    expect(open).not.toHaveBeenCalled();
    expect(go).not.toHaveBeenCalled();
  });

  it("opens the sign-in in a popup that ends at the page that closes itself, and keeps this page", () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    const { popup, open } = fakePopup();
    expect(openSignIn()).toBe("popup");
    expect(open).toHaveBeenCalledTimes(1);
    expect(popup.location.href).toBe(`/oauth2/start?rd=${encodeURIComponent(SIGNED_IN_PATH)}`);
    // the issuer's page cannot reach back to this one
    expect(popup.opener).toBeNull();
    expect(go).not.toHaveBeenCalled();
  });

  it("leaves the page for the sign-in only when the browser refuses the popup, and not twice in a row", () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    vi.spyOn(window, "open").mockReturnValue(null);
    expect(openSignIn(1_000)).toBe("redirect");
    expect(go).toHaveBeenCalledWith("/oauth2/start?rd=%2Fthreads%2Fabc%3Ftab%3Dsources%23m-3");
    // the loop guard of a sign-in that does not help
    expect(openSignIn(1_000 + REDIRECT_PAUSE_MS - 1)).toBe("paused");
    expect(go).toHaveBeenCalledTimes(1);
    expect(openSignIn(1_000 + REDIRECT_PAUSE_MS)).toBe("redirect");
  });

  it("takes a popup that throws for a refused one", () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    vi.spyOn(window, "open").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(openSignIn()).toBe("redirect");
  });
});

describe("signing in where the web holds its own tokens (ADR 0054)", () => {
  beforeEach(() => {
    setBrowserAuth({ issuer: "https://id.example", clientId: "web", scope: "openid" });
    // the edge's sign-in is built into the image too: the deployment's own answer wins
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
  });

  it("opens the issuer in a popup that the callback closes, and keeps this page", () => {
    const start = vi.spyOn(signIn, "startSignIn").mockResolvedValue();
    const popup = { opener: {}, location: { href: "" }, close: vi.fn() };
    vi.spyOn(window, "open").mockReturnValue(popup as unknown as Window);
    expect(openSignIn()).toBe("popup");
    expect(start).toHaveBeenCalledWith({ popup, returnTo: "/" });
    expect(popup.opener).toBeNull();
    expect(go).not.toHaveBeenCalled();
  });

  it("leaves the page for the issuer, and back to this page, only when the popup is refused, not twice in a row", () => {
    const start = vi.spyOn(signIn, "startSignIn").mockResolvedValue();
    vi.spyOn(window, "open").mockReturnValue(null);
    expect(openSignIn(1_000)).toBe("redirect");
    expect(start).toHaveBeenCalledWith({ returnTo: "/threads/abc?tab=sources#m-3" });
    expect(openSignIn(1_000 + REDIRECT_PAUSE_MS - 1)).toBe("paused");
    expect(start).toHaveBeenCalledTimes(1);
  });

  it("sends a reader of a share link, whose link is not public, to the issuer too", () => {
    const start = vi.spyOn(signIn, "startSignIn").mockResolvedValue();
    expect(redirectToSignIn()).toBe(true);
    expect(start).toHaveBeenCalledTimes(1);
    expect(go).not.toHaveBeenCalled();
  });

  it("closes a popup the issuer could not be reached from", async () => {
    vi.spyOn(signIn, "startSignIn").mockRejectedValue(new Error("down"));
    const popup = { opener: {}, location: { href: "" }, close: vi.fn() };
    vi.spyOn(window, "open").mockReturnValue(popup as unknown as Window);
    openSignIn();
    await vi.waitFor(() => expect(popup.close).toHaveBeenCalled());
  });
});
