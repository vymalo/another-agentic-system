// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiUserToolServer } from "@/lib/api/types";
import type { UserToolServersView } from "../../hooks/use-user-tool-servers";

// the apps open the system browser through the native side: a stand-in for it
const desktop = vi.hoisted(() => ({ native: false, open: vi.fn() }));
vi.mock("@/lib/auth/desktop", () => ({
  usesNativeBrowser: () => desktop.native,
  openInBrowser: desktop.open,
}));

import { ServerRow } from "./server-row";

afterEach(cleanup);

// Radix measures its popper content; jsdom has no layout
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const ALL = ["none", "bearer", "api_key", "oauth2"] as const;
const SECRET = "tok-9f3a-this-is-a-secret-value";
const AUTHORIZE_URL = "https://auth.stranger.example/authorize?state=abc123";

const server = (over: Partial<ApiUserToolServer> = {}): ApiUserToolServer => ({
  id: "u-abcdefghij",
  name: "Search",
  own: true,
  status: "ready",
  url: "https://mcp.example.com/secret-path/mcp",
  auth: { kind: "bearer" },
  state: "connected",
  hasCredential: true,
  revision: "3",
  ...over,
});

const ok = <T,>(value: T) => ({ ok: true as const, value });
const refuse = (message: string, reread = false) => ({
  ok: false as const,
  failure: { message, reread },
});

const actions = () => ({
  update: vi.fn<UserToolServersView["update"]>(async () => ok(server())),
  remove: vi.fn<UserToolServersView["remove"]>(async () => ok(null)),
  authorize: vi.fn<UserToolServersView["authorize"]>(async () =>
    ok({
      authorizeUrl: AUTHORIZE_URL,
      issuer: "https://login.stranger.example",
      expiresAt: new Date(Date.now() + 600_000).toISOString(),
    }),
  ),
  reload: vi.fn(),
});

function renderRow(s: ApiUserToolServer, over: { canChange?: boolean } = {}) {
  const a = actions();
  const utils = render(
    <ul>
      <ServerRow server={s} kinds={ALL} canChange={over.canChange ?? true} actions={a} />
    </ul>,
  );
  return { ...utils, a };
}

let open: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  open = vi.spyOn(window, "open").mockImplementation(() => null);
  desktop.native = false;
  desktop.open.mockReset().mockResolvedValue(undefined);
});
afterEach(() => vi.restoreAllMocks());

describe("ServerRow: what a row says", () => {
  it("names the server, its kind and its host, and never the path of its address", () => {
    renderRow(server());
    expect(screen.getByRole("heading", { name: "Search" })).toBeTruthy();
    const text = document.body.textContent ?? "";
    expect(text).toContain("Bearer token");
    expect(text).toContain("mcp.example.com");
    expect(text).not.toContain("secret-path");
  });

  it("says the status in words with an icon beside them, and what to do when the server needs the person", () => {
    renderRow(
      server({
        status: "needs_auth",
        reason: "reauthorize",
        state: "needs_reauth",
        hasCredential: true,
      }),
    );
    const status = document.querySelector('[data-slot="status"]') as HTMLElement;
    expect(status.textContent).toContain("Needs a key.");
    expect(status.textContent).toMatch(/key was refused or ended/);
    expect(status.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
  });
});

describe("ServerRow: what the person can do", () => {
  it("offers Connect to an OAuth server that needs a sign-in, and Add key to one that needs a key", () => {
    const { unmount } = renderRow(
      server({
        auth: { kind: "oauth2" },
        status: "needs_auth",
        reason: "not_connected",
        state: "added",
        hasCredential: false,
      }),
    );
    expect(screen.getByRole("button", { name: "Connect Search" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Add a key/ })).toBeNull();
    unmount();
    renderRow(
      server({
        status: "needs_auth",
        reason: "not_connected",
        state: "added",
        hasCredential: false,
      }),
    );
    expect(screen.getByRole("button", { name: "Add a key to Search" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Connect/ })).toBeNull();
  });

  it("offers only Delete to a person whose roles do not hold the permission", () => {
    renderRow(server(), { canChange: false });
    expect(screen.getByRole("button", { name: "Delete Search" })).toBeTruthy();
    for (const name of [/^Edit/, /^Replace/, /^Connect/, /^Add a key/]) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
  });

  it("renames with the revision the row was read at and only what differs", async () => {
    const { a } = renderRow(server());
    fireEvent.click(screen.getByRole("button", { name: "Edit Search" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Renamed" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(a.update).toHaveBeenCalledTimes(1));
    expect(a.update).toHaveBeenCalledWith("u-abcdefghij", { name: "Renamed", revision: "3" });
    // the form is closed when the change was made
    await waitFor(() => expect(screen.queryByLabelText("Name")).toBeNull());
  });

  it("sends nothing for an edit that changes nothing", async () => {
    const { a } = renderRow(server());
    fireEvent.click(screen.getByRole("button", { name: "Edit Search" }));
    fireEvent.change(screen.getByLabelText("New token"), { target: { value: "" } });
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Search" } });
    // nothing differs: the button is there and sends no request
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(screen.queryByLabelText("Name")).toBeNull());
    expect(a.update).not.toHaveBeenCalled();
  });

  it("re-keys with the secret and the revision alone, and the secret is gone from the page after", async () => {
    const { a } = renderRow(server());
    fireEvent.click(screen.getByRole("button", { name: "Replace the key of Search" }));
    fireEvent.change(screen.getByLabelText("New token"), { target: { value: SECRET } });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));
    await waitFor(() => expect(a.update).toHaveBeenCalledTimes(1));
    expect(a.update).toHaveBeenCalledWith("u-abcdefghij", { secret: SECRET, revision: "3" });
    await waitFor(() => expect(screen.queryByLabelText("New token")).toBeNull());
    expect(document.body.innerHTML).not.toContain(SECRET);
  });

  it("says a refused change in the words it was given and keeps the form open", async () => {
    const { a } = renderRow(server());
    a.update.mockResolvedValueOnce(refuse("This server was changed since you opened it."));
    fireEvent.click(screen.getByRole("button", { name: "Edit Search" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Mine" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "This server was changed since you opened it.",
    );
    expect(screen.getByLabelText("Name")).toBeTruthy();
  });

  it("asks before it deletes, says what goes, deletes by id and closes", async () => {
    const { a } = renderRow(server());
    fireEvent.click(screen.getByRole("button", { name: "Delete Search" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain("Delete “Search”?");
    expect(dialog.textContent).toContain("forgets its key or sign-in");
    expect(a.remove).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(a.remove).toHaveBeenCalledWith("u-abcdefghij"));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("says why a server could not be deleted and leaves the dialog open", async () => {
    const { a } = renderRow(server());
    a.remove.mockResolvedValueOnce(refuse("The credential could not be kept or removed just now."));
    fireEvent.click(screen.getByRole("button", { name: "Delete Search" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() =>
      expect(within(dialog).getByRole("alert").textContent).toContain(
        "could not be kept or removed",
      ),
    );
    expect(screen.queryByRole("alertdialog")).not.toBeNull();
  });
});

describe("ServerRow: Connect (OAuth)", () => {
  const needs = () =>
    server({
      auth: { kind: "oauth2" },
      status: "needs_auth",
      reason: "not_connected",
      state: "added",
      hasCredential: false,
    });

  it("shows where the person is about to be sent BEFORE it opens anything, then opens the link alone, with no opener", async () => {
    const { a } = renderRow(needs());
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(a.authorize).toHaveBeenCalledWith("u-abcdefghij");
    expect(dialog.textContent).toContain("login.stranger.example");
    expect(dialog.textContent).toContain("https://login.stranger.example");
    expect(dialog.textContent).toMatch(/chosen by the server you added, not by this deployment/);
    // nothing is opened by showing it, and the link is not drawn as text or an anchor
    expect(open).not.toHaveBeenCalled();
    expect(document.body.innerHTML).not.toContain("abc123");
    expect(dialog.querySelector("a")).toBeNull();
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Continue to login.stranger.example" }),
    );
    expect(open).toHaveBeenCalledWith(AUTHORIZE_URL, "_blank", "noopener,noreferrer");
    // the dialog is gone, the link with it, and the row waits for the person to come back
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(document.body.innerHTML).not.toContain("abc123");
    expect(screen.getByRole("status").textContent).toContain(
      "Finish signing in in the other window",
    );
    fireEvent.click(screen.getByRole("button", { name: "Check now" }));
    expect(a.reload).toHaveBeenCalled();
  });

  it("opens nothing when the person cancels", async () => {
    renderRow(needs());
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(open).not.toHaveBeenCalled();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("does not let a link that has ended be continued", async () => {
    const { a } = renderRow(needs());
    a.authorize.mockResolvedValueOnce(
      ok({
        authorizeUrl: AUTHORIZE_URL,
        issuer: "https://login.stranger.example",
        expiresAt: new Date(Date.now() - 1000).toISOString(),
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain("This link has ended");
    expect(
      (within(dialog).getByRole("button", { name: /^Continue to/ }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("says why the sign-in could not be started and opens nothing", async () => {
    const { a } = renderRow(needs());
    a.authorize.mockResolvedValueOnce(refuse("This deployment cannot sign in to servers yet."));
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "cannot sign in to servers yet",
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(open).not.toHaveBeenCalled();
  });

  it("opens the system browser through the app in the desktop and mobile apps, never a window of the webview", async () => {
    desktop.native = true;
    renderRow(needs());
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: /^Continue to/ }));
    await waitFor(() => expect(desktop.open).toHaveBeenCalledWith(AUTHORIZE_URL));
    expect(open).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(screen.getByRole("status").textContent).toContain("Finish signing in");
  });

  it("says so, and stays, when the app cannot open the sign-in (its native side opens only its own issuer's addresses)", async () => {
    desktop.native = true;
    desktop.open.mockRejectedValueOnce("only an address of the issuer is opened");
    renderRow(needs());
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: /^Continue to/ }));
    await waitFor(() =>
      expect(within(dialog).getByRole("alert").textContent).toContain(
        "Connect this server from the web app",
      ),
    );
    expect(open).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).not.toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("stops waiting once the server reads ready", async () => {
    const { a, rerender } = renderRow(needs());
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: /^Continue to/ }));
    await screen.findByRole("status");
    rerender(
      <ul>
        <ServerRow
          server={server({ auth: { kind: "oauth2" }, revision: "4" })}
          kinds={ALL}
          canChange
          actions={a}
        />
      </ul>,
    );
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.getByText("Ready.")).toBeTruthy();
  });
});
