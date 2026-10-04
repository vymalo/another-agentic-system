// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { navigation, resetRedirectPause } from "@/lib/api/session";
import { renewSession, resetSessionState, SIGNED_IN_CHANNEL } from "@/lib/api/session-refresh";
import { SessionBanner, SIGN_IN_PAUSED_TEXT } from "./session-banner";
import { SignedIn } from "./signed-in";

/*
 * What the person sees when the session has ended: one line with one button that opens the sign-in in a
 * popup, gone when the edge has a session again; and the popup's last page, which says so and closes.
 */

let go: ReturnType<typeof vi.spyOn>;
/** What the edge answers the browser's question about the session. */
let session = false;
beforeEach(() => {
  resetSessionState();
  resetRedirectPause();
  session = false;
  vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
  go = vi.spyOn(navigation, "go").mockImplementation(() => {});
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response("{}", { status: session ? 200 : 401 })),
  );
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const banner = () => document.querySelector('[data-slot="session-banner"]');
const ended = async () => {
  await act(async () => {
    await renewSession();
  });
};

describe("SessionBanner", () => {
  it("is nothing while there is a session, and says it has ended when there is none", async () => {
    render(<SessionBanner />);
    expect(banner()).toBeNull();
    await ended();
    expect(banner()?.textContent).toContain("Your session has ended");
    expect(screen.getByRole("status").textContent).toContain("Sign in again to go on");
    // a status, not an alert: the page's own error lines are the alerts
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("opens the sign-in in a popup, and goes when the edge has a session again", async () => {
    const popup = { opener: {} as unknown, location: { href: "" } };
    const open = vi.spyOn(window, "open").mockReturnValue(popup as unknown as Window);
    render(<SessionBanner />);
    await ended();
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(open).toHaveBeenCalledTimes(1);
    expect(popup.location.href).toBe("/oauth2/start?rd=%2Fsigned-in");
    expect(go).not.toHaveBeenCalled();

    // the popup's last page says so: the banner asks the edge, and is gone
    session = true;
    await act(async () => {
      const channel = new BroadcastChannel(SIGNED_IN_CHANNEL);
      channel.postMessage("signed-in");
      channel.close();
      await vi.waitFor(() => expect(banner()).toBeNull());
    });
  });

  it("does not take the popup's word for it: a message with no session behind it changes nothing", async () => {
    render(<SessionBanner />);
    await ended();
    const asked = vi.mocked(fetch).mock.calls.length;
    await act(async () => {
      const channel = new BroadcastChannel(SIGNED_IN_CHANNEL);
      channel.postMessage("signed-in");
      channel.close();
      await vi.waitFor(() => expect(vi.mocked(fetch).mock.calls.length).toBe(asked + 1));
    });
    expect(banner()).not.toBeNull();
  });

  it("asks when the window comes back, which is how a sign-in in another tab is found", async () => {
    render(<SessionBanner />);
    await ended();
    session = true;
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
      await vi.waitFor(() => expect(banner()).toBeNull());
    });
  });

  it("leaves the page for the sign-in only when the popup is refused, and says so when that is held back", async () => {
    vi.spyOn(window, "open").mockReturnValue(null);
    render(<SessionBanner />);
    await ended();
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(go).toHaveBeenCalledTimes(1);
    // back from a sign-in that fixed nothing, at once: the pause holds the redirect and the banner says it
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(go).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("status").textContent).toContain(SIGN_IN_PAUSED_TEXT);
  });
});

describe("SignedIn", () => {
  it("tells the waiting tabs and closes the window", async () => {
    const heard: unknown[] = [];
    const listener = new BroadcastChannel(SIGNED_IN_CHANNEL);
    listener.onmessage = (e) => heard.push(e.data);
    const close = vi.spyOn(window, "close").mockImplementation(() => {});
    render(<SignedIn />);
    await vi.waitFor(() => expect(heard).toEqual(["signed-in"]));
    expect(close).toHaveBeenCalled();
    listener.close();
  });

  it("a window that cannot close itself says so, with a way back", async () => {
    vi.spyOn(window, "close").mockImplementation(() => {});
    render(<SignedIn />);
    expect(await screen.findByRole("link", { name: "Open the chat" })).toBeTruthy();
    expect(screen.getByText(/You can close this window/)).toBeTruthy();
  });
});
