// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { MeState } from "@/features/me/hooks/use-me";
import type { ApiMe } from "@/lib/api/types";
import { setBrowserAuth } from "@/lib/auth/config";
import { navigation } from "@/lib/auth/navigation";

/*
 * The account menu at the foot of the sidebar, and the no-access screen's button: who the person is and
 * **Sign out**, which in browser mode revokes and deletes (`lib/auth/sign-out.ts`) and with an edge goes
 * to oauth2-proxy's sign-out, back to the start page.
 */
const state = vi.hoisted(() => ({
  me: { status: "loading", me: null } as MeState,
  signOut: vi.fn(async () => {}),
}));
vi.mock("@/features/me/hooks/use-me", () => ({ useMe: () => state.me }));
vi.mock("@/lib/auth/sign-out", () => ({ signOut: state.signOut, isSigningOut: () => false }));

import { AccountMenu, SIGN_OUT_LABEL, SignOutButton } from "./account-menu";

// Radix measures its popper content; jsdom has no layout
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const ME: ApiMe = {
  user: "ada@example.com",
  email: "Ada@Example.com",
  name: "Ada Lovelace",
  roles: ["user"],
  permissions: [],
  agents: { read: ["*"], invoke: ["*"] },
};

let go: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  state.me = { status: "ready", me: ME };
  state.signOut.mockClear();
  go = vi.spyOn(navigation, "go").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  setBrowserAuth(undefined);
});

async function openMenu() {
  const trigger = await screen.findByRole("button", { name: "Account: Ada@Example.com" });
  fireEvent.keyDown(trigger, { key: "Enter" });
  return screen.findByRole("menu");
}

describe("the account menu", () => {
  it("names the person and, with an edge, signs out at oauth2-proxy and comes back to the start page", async () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    setBrowserAuth(null);
    render(<AccountMenu />, { wrapper: TooltipProvider });
    const menu = await openMenu();
    expect(menu.textContent).toContain("Signed in as Ada@Example.com");
    fireEvent.click(screen.getByRole("menuitem", { name: SIGN_OUT_LABEL }));
    expect(go).toHaveBeenCalledWith("/oauth2/sign_out?rd=%2F");
    expect(state.signOut).not.toHaveBeenCalled();
  });

  it("signs out of the browser's own session in browser mode, and never through the edge", async () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    setBrowserAuth({ issuer: "https://id.example", clientId: "web", scope: "openid" });
    render(<AccountMenu />, { wrapper: TooltipProvider });
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: SIGN_OUT_LABEL }));
    await waitFor(() => expect(state.signOut).toHaveBeenCalledTimes(1));
    expect(go).not.toHaveBeenCalled();
  });

  it("has no Sign out where there is no sign-in to end, and nothing at all without a person", async () => {
    setBrowserAuth(null);
    const { container, rerender } = render(<AccountMenu />, { wrapper: TooltipProvider });
    const menu = await openMenu();
    expect(menu.textContent).toContain("Signed in as Ada@Example.com");
    expect(screen.queryByRole("menuitem", { name: SIGN_OUT_LABEL })).toBeNull();
    fireEvent.keyDown(menu, { key: "Escape" });
    state.me = { status: "unknown", me: null };
    rerender(<AccountMenu />);
    expect(container.querySelector('[data-slot="account"]')).toBeNull();
  });

  it("waits for the deployment's kind before it offers Sign out", () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    state.me = { status: "unknown", me: null };
    setBrowserAuth(undefined);
    vi.stubGlobal(
      "fetch",
      vi.fn(() => new Promise<Response>(() => {})),
    );
    const { container } = render(<AccountMenu />, { wrapper: TooltipProvider });
    expect(container.querySelector('[data-slot="account"]')).toBeNull();
    vi.unstubAllGlobals();
  });
});

describe("the no-access screen's Sign out", () => {
  it("is a button with an edge or in browser mode, and nothing otherwise", async () => {
    setBrowserAuth(null);
    const { rerender } = render(<SignOutButton />);
    expect(screen.queryByRole("button", { name: SIGN_OUT_LABEL })).toBeNull();
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    rerender(<SignOutButton key="edge" />);
    fireEvent.click(await screen.findByRole("button", { name: SIGN_OUT_LABEL }));
    expect(go).toHaveBeenCalledWith("/oauth2/sign_out?rd=%2F");
    expect(screen.getByRole("button", { name: SIGN_OUT_LABEL })).toHaveProperty("disabled", true);
  });
});
