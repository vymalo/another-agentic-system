// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  navigation,
  REDIRECT_PAUSE_MS,
  redirectToSignIn,
  resetRedirectPause,
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
