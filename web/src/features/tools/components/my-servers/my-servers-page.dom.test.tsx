// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MeState } from "@/features/me/hooks/use-me";
import type { ApiMe, ApiUserToolServer } from "@/lib/api/types";

/*
 * `/tools` (ADR 0046): the person's own MCP servers. The server enforces everything; the page offers Add only to a
 * holder of `tools.user.add` below the limit, says in words what a person without it, a deployment with the feature
 * off and a list that cannot be read are told, and never shows a secret.
 */
const client = vi.hoisted(() => ({ GET: vi.fn(), POST: vi.fn(), PATCH: vi.fn(), DELETE: vi.fn() }));
vi.mock("@/lib/api/client", async () => ({
  ...(await vi.importActual<typeof import("@/lib/api/client")>("@/lib/api/client")),
  api: client,
}));
const me = vi.hoisted(() => ({ state: { status: "loading", me: null } as MeState }));
vi.mock("@/features/me/hooks/use-me", () => ({ useMe: () => me.state }));
vi.mock("@/features/session/components/account-menu", () => ({
  TOOL_SERVERS_LABEL: "My tool servers",
}));

import { MyServersPage } from "./my-servers-page";

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const ME: ApiMe = {
  user: "maker@example.com",
  roles: ["tool-maker"],
  permissions: [{ permission: "thread.write", scope: "own" }, { permission: "tools.user.add" }],
  agents: { read: ["*"], invoke: ["*"] },
};
const SECRET = "tok-9f3a-this-is-a-secret-value";

const row = (over: Partial<ApiUserToolServer> = {}): ApiUserToolServer => ({
  id: "u-abcdefghij",
  name: "Search",
  own: true,
  status: "ready",
  url: "https://mcp.example.com/mcp",
  auth: { kind: "bearer" },
  state: "connected",
  hasCredential: true,
  revision: "1",
  ...over,
});
const listing = (servers: ApiUserToolServer[], extra: object = {}) => ({
  data: { servers, limit: 8, kinds: ["none", "bearer", "api_key", "oauth2"], ...extra },
  response: { status: 200, ok: true, headers: new Headers() },
});
const refused = (status: number, error: object = { title: "x", status }) => ({
  error,
  response: { status, ok: false, headers: new Headers() },
});

beforeEach(() => {
  me.state = { status: "ready", me: ME };
  client.GET.mockReset().mockResolvedValue(listing([row()]));
  client.POST.mockReset();
  client.PATCH.mockReset();
  client.DELETE.mockReset();
});
afterEach(cleanup);

const heading = (name: string) => screen.findByRole("heading", { level: 3, name });

describe("MyServersPage", () => {
  it("lists the person's servers with how many of how many, and a button to add another", async () => {
    render(<MyServersPage />);
    expect(screen.getByRole("heading", { level: 1, name: "My tool servers" })).toBeTruthy();
    expect(await heading("Search")).toBeTruthy();
    expect(screen.getByRole("heading", { level: 2, name: /Your servers/ }).textContent).toContain(
      "1 of 8",
    );
    // the form is behind the button once there is a server: the page is the list first
    expect(screen.queryByLabelText("Address")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Add a server" }));
    expect(screen.getByLabelText("Address")).toBeTruthy();
  });

  it("shows the form at once when there is nothing yet", async () => {
    client.GET.mockResolvedValue(listing([]));
    render(<MyServersPage />);
    expect(await screen.findByText("You have not added a server yet.")).toBeTruthy();
    expect(screen.getByLabelText("Address")).toBeTruthy();
  });

  it("adds a server: the secret goes in the request once, and is in the page nowhere after", async () => {
    client.GET.mockResolvedValue(listing([]));
    client.POST.mockResolvedValueOnce({
      data: row({ id: "u-aaaaaaaaaa", name: "Mine" }),
      response: { status: 201, ok: true, headers: new Headers() },
    });
    render(<MyServersPage />);
    await screen.findByLabelText("Address");
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Mine" } });
    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "https://mcp.example.com/mcp" },
    });
    fireEvent.change(screen.getByLabelText("Authentication"), { target: { value: "bearer" } });
    fireEvent.change(screen.getByLabelText("Token"), { target: { value: SECRET } });
    fireEvent.click(screen.getByRole("button", { name: "Add server" }));
    await waitFor(() => expect(client.POST).toHaveBeenCalledTimes(1));
    expect(client.POST.mock.calls[0]?.[1]).toEqual({
      body: {
        name: "Mine",
        url: "https://mcp.example.com/mcp",
        auth: { kind: "bearer" },
        secret: SECRET,
      },
    });
    expect(await screen.findByText("Added “Mine”.")).toBeTruthy();
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(JSON.stringify(window.localStorage)).not.toContain(SECRET);
    expect(window.location.href).not.toContain(SECRET);
  });

  it("offers only the kinds the deployment listed", async () => {
    client.GET.mockResolvedValue(listing([], { kinds: ["none"] }));
    render(<MyServersPage />);
    const select = (await screen.findByLabelText("Authentication")) as HTMLSelectElement;
    expect([...select.options].map((o) => o.value)).toEqual(["none"]);
  });

  it("says in words that a person without tools.user.add cannot add or change, and offers them only Delete", async () => {
    me.state = {
      status: "ready",
      me: { ...ME, permissions: [{ permission: "thread.write", scope: "own" }] },
    };
    render(<MyServersPage />);
    await heading("Search");
    expect(
      screen.getByText(/Your roles do not let you add or change tool servers of your own/),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Add a server" })).toBeNull();
    expect(screen.queryByLabelText("Address")).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit Search" })).toBeNull();
    expect(screen.getByRole("button", { name: "Delete Search" })).toBeTruthy();
  });

  it("hides nothing while who the person is cannot be known: the server decides and says so in words", async () => {
    me.state = { status: "unknown", me: null };
    render(<MyServersPage />);
    await heading("Search");
    expect(screen.getByRole("button", { name: "Add a server" })).toBeTruthy();
    expect(screen.queryByText(/Your roles do not let you add/)).toBeNull();
  });

  it("says a refused add in the words of its code, from a server that says the person's permission is gone", async () => {
    client.GET.mockResolvedValue(listing([]));
    client.POST.mockResolvedValueOnce(
      refused(403, { title: "Forbidden", status: 403, code: "forbidden" }),
    );
    render(<MyServersPage />);
    await screen.findByLabelText("Address");
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Mine" } });
    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "https://mcp.example.com/mcp" },
    });
    fireEvent.change(screen.getByLabelText("Authentication"), { target: { value: "none" } });
    fireEvent.click(screen.getByRole("button", { name: "Add server" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(
      /roles do not let you add or change/,
    );
  });

  it("stops offering Add at the limit, in words", async () => {
    const eight = Array.from({ length: 8 }, (_, i) =>
      row({ id: `u-${String(i).repeat(10)}`, name: `Server ${i}` }),
    );
    client.GET.mockResolvedValue(listing(eight));
    render(<MyServersPage />);
    await heading("Server 0");
    expect(screen.getByText(/You have 8 servers, the most one person can have/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Add a server" })).toBeNull();
    expect(screen.queryByLabelText("Address")).toBeNull();
  });

  it("says when the deployment has the feature off, when roles grant nothing and when the list cannot be read", async () => {
    client.GET.mockResolvedValueOnce(refused(404));
    const off = render(<MyServersPage />);
    expect(
      await screen.findByText(/does not let people add tool servers of their own/),
    ).toBeTruthy();
    off.unmount();
    client.GET.mockResolvedValueOnce(
      refused(403, { title: "Forbidden", status: 403, code: "no_access" }),
    );
    const none = render(<MyServersPage />);
    expect(await screen.findByText(/Your roles grant no access to this app/)).toBeTruthy();
    none.unmount();
    client.GET.mockResolvedValueOnce(listing([], { error: { code: "unreadable", detail: "x" } }));
    render(<MyServersPage />);
    expect(await screen.findByText(/One of your servers could not be read/)).toBeTruthy();
  });

  it("says a failed read, with a way to try again", async () => {
    client.GET.mockResolvedValueOnce(
      refused(503, { title: "x", status: 503, detail: "Storage is unavailable" }),
    );
    render(<MyServersPage />);
    expect((await screen.findByRole("alert")).textContent).toContain("Storage is unavailable");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await heading("Search")).toBeTruthy();
  });
});
