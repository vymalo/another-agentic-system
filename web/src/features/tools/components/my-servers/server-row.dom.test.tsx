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
  const flowAt = (authorizeUrl: string, issuer = "https://login.stranger.example") =>
    ok({ authorizeUrl, issuer, expiresAt: new Date(Date.now() + 600_000).toISOString() });
  /** The link in the dialog: a real link, so a pop-up blocker has nothing to block. Its default navigation is jsdom's to refuse. */
  const link = (dialog: HTMLElement) => {
    const a = within(dialog).getByRole("link", { name: /^Continue to/ });
    a.addEventListener("click", (event) => event.preventDefault());
    return a as HTMLAnchorElement;
  };

  it("shows where the person is about to be sent BEFORE it opens anything, and the link it offers goes to exactly that host", async () => {
    const { a } = renderRow(needs());
    a.authorize.mockResolvedValueOnce(
      flowAt("https://login.stranger.example/authorize?state=abc123"),
    );
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(a.authorize).toHaveBeenCalledWith("u-abcdefghij");
    // the sentence names the destination, and it is the host of the link that is offered
    const sentence = dialog.querySelector('[data-slot="destination"]')?.textContent;
    expect(sentence).toBe("login.stranger.example");
    expect(dialog.textContent).toContain("You will be sent to login.stranger.example");
    expect(dialog.textContent).toMatch(/chosen by the server you added, not by this deployment/);
    expect(dialog.textContent).toContain("https://login.stranger.example");
    const go = link(dialog);
    expect(new URL(go.href).host).toBe(sentence);
    expect(go.getAttribute("target")).toBe("_blank");
    expect(go.getAttribute("rel")).toBe("noopener noreferrer");
    expect(go.textContent).toBe("Continue to login.stranger.example");
    // nothing was opened by showing it, and after the dialog the link is in no markup
    expect(open).not.toHaveBeenCalled();
    fireEvent.click(go);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(document.body.innerHTML).not.toContain("abc123");
  });

  it("names both hosts when the authorization endpoint is not on the issuer's host, and sends the person to the real one", async () => {
    const { a } = renderRow(needs());
    a.authorize.mockResolvedValueOnce(flowAt("https://accounts.other.example/o/authorize?state=s"));
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.querySelector('[data-slot="destination"]')?.textContent).toBe(
      "accounts.other.example",
    );
    const hosts = dialog.querySelector('[data-slot="hosts"]')?.textContent ?? "";
    expect(hosts).toContain("login.stranger.example");
    expect(hosts).toContain("accounts.other.example");
    expect(new URL(link(dialog).href).host).toBe("accounts.other.example");
  });

  it("opens nothing, and says the server gave a link that cannot be opened, for javascript:, data:, http, a relative and a malformed link", async () => {
    for (const bad of [
      "javascript:alert(document.domain)",
      "data:text/html,<script>alert(1)</script>",
      "http://example.com/authorize",
      "/authorize?state=s",
      "not a url",
    ]) {
      const { a, unmount } = renderRow(needs());
      a.authorize.mockResolvedValueOnce(flowAt(bad));
      fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
      const dialog = await screen.findByRole("alertdialog");
      expect(dialog.textContent, bad).toContain("gave a sign-in link that cannot be opened");
      expect(within(dialog).queryByRole("link"), bad).toBeNull();
      expect(dialog.querySelector("a"), bad).toBeNull();
      expect(
        (within(dialog).getByRole("button", { name: "Continue" }) as HTMLButtonElement).disabled,
        bad,
      ).toBe(true);
      expect(open, bad).not.toHaveBeenCalled();
      expect(desktop.open, bad).not.toHaveBeenCalled();
      expect(document.body.innerHTML, bad).not.toContain("javascript:");
      unmount();
    }
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
    expect(within(dialog).queryByRole("link")).toBeNull();
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

  it("does not claim a window opened: the row says it is waiting, true whether or not a pop-up was blocked, and offers the page again", async () => {
    const { a } = renderRow(needs());
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(link(dialog));
    const waiting = await screen.findByRole("status");
    expect(waiting.textContent).toContain("Waiting for you to finish signing in");
    expect(waiting.textContent).not.toMatch(/other window/);
    fireEvent.click(within(waiting).getByRole("button", { name: "Check now" }));
    expect(a.reload).toHaveBeenCalled();
    // a new flow replaces the old one: a new dialog, with a new link
    fireEvent.click(within(waiting).getByRole("button", { name: "Open the sign-in page again" }));
    await waitFor(() => expect(a.authorize).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole("alertdialog")).toBeTruthy();
  });

  it("opens the system browser through the app in the desktop and mobile apps, and never a link of the webview", async () => {
    desktop.native = true;
    renderRow(needs());
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).queryByRole("link")).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: /^Continue to/ }));
    await waitFor(() => expect(desktop.open).toHaveBeenCalledWith(AUTHORIZE_URL));
    expect(open).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(screen.getByRole("status").textContent).toContain("Waiting for you");
  });

  it("gives the app nothing to open for a link that cannot be opened", async () => {
    desktop.native = true;
    const { a } = renderRow(needs());
    a.authorize.mockResolvedValueOnce(flowAt("javascript:alert(1)"));
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(
      (within(dialog).getByRole("button", { name: "Continue" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(desktop.open).not.toHaveBeenCalled();
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
    expect(screen.queryByRole("alertdialog")).not.toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("stops waiting once the server reads ready, with the same revision or a new one", async () => {
    const { a, rerender } = renderRow(needs());
    fireEvent.click(screen.getByRole("button", { name: "Connect Search" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(link(dialog));
    await screen.findByRole("status");
    const again = (revision: string) =>
      rerender(
        <ul>
          <ServerRow
            server={server({ auth: { kind: "oauth2" }, revision })}
            kinds={ALL}
            canChange
            actions={a}
          />
        </ul>,
      );
    // ready under the revision the row had when the person went: not waiting any more (the status says it)
    again(needs().revision);
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.getByText("Ready.")).toBeTruthy();
    again("4");
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("ServerRow: a form opened on a row keeps that row", () => {
  it("sends the revision it was opened on, and the words of a 409 when the server has moved, so a re-read never lets old values overwrite a newer change", async () => {
    const { a, rerender } = renderRow(server({ name: "Search", revision: "3" }));
    a.update.mockResolvedValueOnce(
      refuse(
        "This server was changed since you opened it. Check it, then make your change again.",
        true,
      ),
    );
    fireEvent.click(screen.getByRole("button", { name: "Edit Search" }));
    // the list is read again behind the open form: another tab renamed the server
    rerender(
      <ul>
        <ServerRow
          server={server({ name: "Renamed elsewhere", revision: "9" })}
          kinds={ALL}
          canChange
          actions={a}
        />
      </ul>,
    );
    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "https://moved.example.com/mcp" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(a.update).toHaveBeenCalledTimes(1));
    // the revision N the form was opened on, and only what differs from the row it was opened on: not a rename back
    expect(a.update).toHaveBeenCalledWith("u-abcdefghij", {
      url: "https://moved.example.com/mcp",
      revision: "3",
    });
    expect((await screen.findByRole("alert")).textContent).toContain("changed since you opened it");
  });

  it("does the same for a new key", async () => {
    const { a, rerender } = renderRow(server({ revision: "3" }));
    fireEvent.click(screen.getByRole("button", { name: "Replace the key of Search" }));
    rerender(
      <ul>
        <ServerRow server={server({ revision: "9" })} kinds={ALL} canChange actions={a} />
      </ul>,
    );
    fireEvent.change(screen.getByLabelText("New token"), { target: { value: SECRET } });
    fireEvent.click(screen.getByRole("button", { name: "Save key" }));
    await waitFor(() =>
      expect(a.update).toHaveBeenCalledWith("u-abcdefghij", { secret: SECRET, revision: "3" }),
    );
  });

  it("ties a refused address to the Address field and moves the focus there", async () => {
    const { a } = renderRow(server());
    a.update.mockResolvedValueOnce({
      ok: false as const,
      failure: { message: "That address was refused. no.", reread: false, field: "url" as const },
    });
    fireEvent.click(screen.getByRole("button", { name: "Edit Search" }));
    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "https://10.0.0.1/mcp" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    const address = screen.getByLabelText("Address") as HTMLInputElement;
    await waitFor(() => expect(address.getAttribute("aria-invalid")).toBe("true"));
    const alert = await screen.findByRole("alert");
    expect(address.getAttribute("aria-describedby")).toContain(alert.parentElement?.id ?? "x");
    expect(document.activeElement).toBe(address);
    // another field is untouched
    expect(screen.getByLabelText("Name").getAttribute("aria-invalid")).toBeNull();
  });
});
