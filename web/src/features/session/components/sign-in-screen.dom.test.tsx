// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBrowserAuth } from "@/lib/auth/config";
import { requireSignIn, resetSignInNeed } from "@/lib/auth/sign-in-need";

/*
 * The app's own sign-in screen (browser mode, ADR 0054): it stands in for the app when this browser holds
 * no usable sign-in, says so when the issuer has refused the one it held, and goes to the issuer only when
 * the person presses Sign in. With an edge it is never shown.
 */
const auth = vi.hoisted(() => ({
  stored: vi.fn(async (): Promise<"usable" | "ended" | "none"> => "none"),
  startSignIn: vi.fn(async (_options: { returnTo?: string }) => {}),
}));
vi.mock("@/lib/auth/tokens", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/auth/tokens")>()),
  storedSession: auth.stored,
}));
vi.mock("@/lib/auth/sign-in", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/auth/sign-in")>()),
  startSignIn: auth.startSignIn,
}));

import { renewSession, resetSessionState } from "@/lib/api/session-refresh";
import { SessionBanner } from "./session-banner";
import {
  ISSUER_UNREACHABLE,
  organisationOf,
  SESSION_ENDED_LINE,
  SIGN_IN_LABEL,
  SignInGate,
} from "./sign-in-screen";

const CONFIG = {
  issuer: "https://auth.example.com/realms/acme",
  clientId: "web",
  scope: "openid",
};
const App = () => <p>the app</p>;

beforeEach(() => {
  resetSignInNeed();
  resetSessionState();
  auth.stored.mockReset();
  auth.stored.mockResolvedValue("none");
  auth.startSignIn.mockReset();
  auth.startSignIn.mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup();
  setBrowserAuth(undefined);
  vi.restoreAllMocks();
});

describe("organisationOf", () => {
  it("is the Keycloak realm, else the issuer's host", () => {
    expect(organisationOf("https://auth.verif.fyi/realms/vymalo")).toEqual({
      name: "vymalo",
      host: "auth.verif.fyi",
    });
    expect(organisationOf("https://id.example.com/")).toEqual({
      name: "id.example.com",
      host: "id.example.com",
    });
    expect(organisationOf("not a url").host).toBe("");
  });
});

describe("the sign-in gate", () => {
  it("is the app with an edge: nothing is asked of this browser", async () => {
    setBrowserAuth(null);
    render(
      <SignInGate>
        <App />
      </SignInGate>,
    );
    expect(screen.getByText("the app")).toBeTruthy();
    expect(auth.stored).not.toHaveBeenCalled();
  });

  it("stands in for the app when nobody is signed in, names the organisation, and goes to the issuer only on the click", async () => {
    setBrowserAuth(CONFIG);
    render(
      <SignInGate>
        <App />
      </SignInGate>,
    );
    const button = await screen.findByRole("button", { name: SIGN_IN_LABEL });
    expect(screen.queryByText("the app")).toBeNull();
    expect(document.body.textContent).toContain(
      "Sign in with your acme account at auth.example.com.",
    );
    expect(screen.queryByText(SESSION_ENDED_LINE)).toBeNull();
    expect(auth.startSignIn).not.toHaveBeenCalled();
    fireEvent.click(button);
    expect(auth.startSignIn).toHaveBeenCalledTimes(1);
    expect(auth.startSignIn.mock.calls[0]?.[0]).toEqual({ returnTo: "/" });
  });

  it("says that the session has ended when the issuer refused the sign-in this browser held", async () => {
    setBrowserAuth(CONFIG);
    auth.stored.mockResolvedValue("ended");
    render(
      <SignInGate>
        <App />
      </SignInGate>,
    );
    expect(await screen.findByText(SESSION_ENDED_LINE)).toBeTruthy();
  });

  it("is the app while the sign-in is usable, and the screen once a request finds nobody signed in", async () => {
    setBrowserAuth(CONFIG);
    auth.stored.mockResolvedValue("usable");
    render(
      <SignInGate>
        <App />
      </SignInGate>,
    );
    await waitFor(() => expect(auth.stored).toHaveBeenCalled());
    expect(screen.getByText("the app")).toBeTruthy();
    act(() => requireSignIn("none"));
    expect(await screen.findByRole("button", { name: SIGN_IN_LABEL })).toBeTruthy();
  });

  it("asks nothing on a share link's page until the link needs a sign-in", async () => {
    setBrowserAuth(CONFIG);
    render(
      <SignInGate check={false}>
        <App />
      </SignInGate>,
    );
    expect(screen.getByText("the app")).toBeTruthy();
    expect(auth.stored).not.toHaveBeenCalled();
    act(() => requireSignIn("none"));
    expect(await screen.findByRole("button", { name: SIGN_IN_LABEL })).toBeTruthy();
  });

  it("says so when the issuer cannot be reached, and lets the person try again", async () => {
    setBrowserAuth(CONFIG);
    auth.startSignIn.mockRejectedValueOnce(new TypeError("network"));
    render(
      <SignInGate>
        <App />
      </SignInGate>,
    );
    const button = await screen.findByRole("button", { name: SIGN_IN_LABEL });
    fireEvent.click(button);
    expect((await screen.findByRole("alert")).textContent).toBe(ISSUER_UNREACHABLE);
    expect(button).toHaveProperty("disabled", false);
    fireEvent.click(button);
    expect(auth.startSignIn).toHaveBeenCalledTimes(2);
  });

  it("hides the session banner while it stands in for the app", async () => {
    setBrowserAuth(CONFIG);
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("{}", { status: 401 })),
    );
    render(
      <>
        <SignInGate>
          <App />
        </SignInGate>
        <SessionBanner />
      </>,
    );
    await screen.findByRole("button", { name: SIGN_IN_LABEL });
    await act(async () => {
      await renewSession();
    });
    expect(document.querySelector('[data-slot="session-banner"]')).toBeNull();
    vi.unstubAllGlobals();
  });
});
