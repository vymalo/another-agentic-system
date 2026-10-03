// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import {
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { uuidv7 } from "@/lib/uuid";
import { createMockServer } from "../../../../mock/server";

/*
 * MCP servers attached to a conversation (ADR 0024), through the whole app: the real mock
 * orchestrator lists the servers, takes `PUT /api/threads/{id}/tools` and `vymalo.tools` on the run
 * that creates a thread, and answers `GET /api/me` for the session the test is. What the page shows
 * is what the server would accept, and what it says (a line, a chip, a flag) is what the log says.
 */

const router = { push: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

let ChatShell: typeof import("./chat-shell").ChatShell;
let resetMe: () => void;
let resetUiConfig: () => void;
let resetRedirectPause: () => void;

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** Every request the app made: `METHOD path`. */
let calls: string[] = [];
/** The bodies of the runs the app started. */
let runs: { threadId: string; forwardedProps: Record<string, unknown> }[] = [];
let cookie = "";

beforeAll(async () => {
  configure({ asyncUtilTimeout: 10_000 });
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollTo = () => {};
  Element.prototype.scrollIntoView = () => {};

  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const abs = (input: unknown) =>
    typeof input === "string" && input.startsWith("/") ? base + input : input;
  globalThis.Request = class extends RealRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(abs(input) as RequestInfo, init);
    }
  } as typeof Request;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : undefined;
    const url = new URL(request ? request.url : String(abs(String(input))));
    const method = request?.method ?? init?.method ?? "GET";
    calls.push(`${method} ${url.pathname}`);
    if (method === "POST" && url.pathname.startsWith("/agui/agents/") && request) {
      runs.push(JSON.parse(await request.clone().text()));
    }
    const carried = new RealRequest(abs(input) as RequestInfo, init);
    if (cookie) carried.headers.set("cookie", cookie);
    return realFetch(carried);
  }) as typeof fetch;
  ({ ChatShell } = await import("./chat-shell"));
  ({ resetMe } = await import("@/features/me/hooks/use-me"));
  ({ resetUiConfig } = await import("@/features/chat/hooks/use-ui-config"));
  ({ resetRedirectPause } = await import("@/lib/api/session"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});

let sessions = 0;
/** Makes the app's requests those of a session that is `profile`. */
async function as(profile: "user" | "admin" | "read-only"): Promise<void> {
  const session = `dom-tools-${++sessions}`;
  await realFetch(`${base}/__mock/config?me=${profile}&session=${session}`, { method: "POST" });
  cookie = `mock-registry=${session}`;
}

beforeEach(async () => {
  await realFetch(`${base}/__mock/reset`, { method: "POST" });
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  window.localStorage.clear();
  window.sessionStorage.clear();
  window.history.pushState({}, "", "/");
  resetMe();
  resetUiConfig();
  resetRedirectPause();
  router.push.mockClear();
  calls = [];
  runs = [];
  cookie = "";
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const shell = (threadId: string | null) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

/** A thread of the default person run to its end; `tools` ride the run that creates it. */
async function makeThread(text: string, tools?: string[], agent = "coder"): Promise<string> {
  const threadId = uuidv7();
  const res = await realFetch(`${base}/agui/agents/${agent}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({
      threadId,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: text }],
      forwardedProps: tools ? { "vymalo.tools": tools } : {},
    }),
  });
  expect(res.status).toBe(200);
  await res.text();
  return threadId;
}

const toolsButton = () => screen.queryByRole("button", { name: "Tools" });
async function openTools(): Promise<HTMLElement> {
  const trigger = await screen.findByRole("button", { name: "Tools" });
  trigger.focus();
  fireEvent.keyDown(trigger, { key: "Enter" });
  return screen.findByRole("menu");
}
const chips = () =>
  [...document.querySelectorAll("[data-slot='tool-chip']")].map((c) => c.textContent ?? "");
const lines = () =>
  [...document.querySelectorAll("[data-slot='tools-line']")].map((c) => c.textContent ?? "");
const warning = () => document.querySelector("[data-slot='tools-warning']")?.textContent ?? null;
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
const composer = () => screen.getByRole("textbox", { name: "Message" });

describe("a new chat", () => {
  it("offers the servers of the deployment, and the run that creates the thread carries the ids chosen", async () => {
    shell(null);
    const menu = await openTools();
    expect(within(menu).getAllByRole("menuitemcheckbox")).toHaveLength(3);
    fireEvent.click(within(menu).getByRole("menuitemcheckbox", { name: /^Web search/ }));
    fireEvent.click(within(menu).getByRole("menuitemcheckbox", { name: /^Team docs/ }));
    fireEvent.keyDown(menu, { key: "Escape" });
    await waitFor(() => expect(chips()).toEqual(["Team docs", "Web search"]));

    fireEvent.change(composer(), { target: { value: "echo with tools" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(router.push).toHaveBeenCalled());
    expect(runs).toHaveLength(1);
    expect(runs[0]?.forwardedProps["vymalo.tools"]).toEqual(["docs", "websearch"]);
    // the thread the orchestrator made has them
    const id = runs[0]?.threadId ?? "";
    const thread = (await (await realFetch(`${base}/api/threads/${id}`)).json()) as {
      tools: string[];
    };
    expect(thread.tools).toEqual(["docs", "websearch"]);
  });

  it("sends no vymalo.tools when nothing is chosen", async () => {
    shell(null);
    await screen.findByRole("button", { name: "Tools" });
    fireEvent.change(composer(), { target: { value: "echo plain" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(router.push).toHaveBeenCalled());
    expect(Object.keys(runs[0]?.forwardedProps ?? {})).not.toContain("vymalo.tools");
  });

  it("flags an agent that cannot use them before anything is sent, and says nothing for the coder", async () => {
    shell(null);
    const menu = await openTools();
    fireEvent.click(within(menu).getByRole("menuitemcheckbox", { name: /^Web search/ }));
    fireEvent.keyDown(menu, { key: "Escape" });
    await waitFor(() => expect(chips()).toEqual(["Web search"]));
    // the coder lists thread-tools/v1; the card is read, and nothing is said
    await waitFor(() => expect(calls).toContain("GET /agui/agents/coder/capabilities"));
    expect(warning()).toBeNull();

    fireEvent.keyDown(await screen.findByRole("button", { name: /^Agent:/ }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitemradio", { name: /^Reviewer/ }));
    await waitFor(() =>
      expect(warning()).toBe("Reviewer cannot use attached tools, so they will not be sent to it."),
    );
    // the choice is kept, and nothing was sent
    expect(chips()).toEqual(["Web search"]);
    expect(runs).toEqual([]);
  });

  it("drops a choice the other agent may not have, as the orchestrator would refuse it", async () => {
    shell(null);
    const menu = await openTools();
    fireEvent.click(within(menu).getByRole("menuitemcheckbox", { name: /^GitHub/ }));
    fireEvent.keyDown(menu, { key: "Escape" });
    await waitFor(() => expect(chips()).toEqual(["GitHub"]));
    fireEvent.keyDown(await screen.findByRole("button", { name: /^Agent:/ }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitemradio", { name: /^Reviewer/ }));
    await waitFor(() => expect(chips()).toEqual([]));
  });
});

describe("a thread", () => {
  it("attaches with one PUT, shows the chip and the line, and detaches the same way", async () => {
    const id = await makeThread("echo nothing yet");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(chips()).toEqual([]);

    const menu = await openTools();
    fireEvent.click(within(menu).getByRole("menuitemcheckbox", { name: /^Team docs/ }));
    await waitFor(() => expect(chips()).toEqual(["Team docs"]));
    expect(calls.filter((c) => c.startsWith("PUT"))).toEqual([`PUT /api/threads/${id}/tools`]);
    await waitFor(() => expect(lines()).toEqual(["Team docs attached"]));

    fireEvent.click(screen.getByRole("button", { name: "Remove Team docs" }));
    await waitFor(() => expect(chips()).toEqual([]));
    await waitFor(() => expect(lines()).toEqual(["Team docs attached", "Team docs detached"]));
    expect(calls.filter((c) => c.startsWith("PUT"))).toHaveLength(2);
  });

  it("starts with the servers the thread has, and names them in the line the log says", async () => {
    const id = await makeThread("echo started with tools", ["websearch"]);
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(chips()).toEqual(["Web search"]));
    expect(lines()).toEqual(["Web search attached"]);
    // a follow-up carries no vymalo.tools: the set is the thread's
    fireEvent.change(composer(), { target: { value: "echo more" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(runs).toHaveLength(1));
    expect(Object.keys(runs[0]?.forwardedProps ?? {})).not.toContain("vymalo.tools");
  });

  it("flags the thread's agent when its card does not list thread-tools/v1", async () => {
    const id = await makeThread("echo reviewed", ["websearch"], "reviewer");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() =>
      expect(warning()).toBe("Reviewer cannot use attached tools, so they will not be sent to it."),
    );
  });
});

describe("a thread the person may read and not change", () => {
  it("has no picker, and no chip, and the line still names what was attached", async () => {
    // a thread of a role that reads and does not write (nobody's else's is readable, ADR 0039)
    const viewed = await makeThread("echo viewed", ["websearch"]);
    await realFetch(`${base}/__mock/owner?thread=${viewed}&owner=viewer@example.com`, {
      method: "POST",
    });
    await as("read-only");
    shell(viewed);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(document.querySelector("[data-slot='read-only']")).not.toBeNull());
    expect(toolsButton()).toBeNull();
    expect(chips()).toEqual([]);
    // the role may not list the servers (that takes `thread.write`), so the line names the id
    await waitFor(() => expect(lines()).toEqual(["websearch attached"]));
  });

  it("does not even ask for the list when no role holds thread.write", async () => {
    await as("read-only");
    shell(null);
    await waitFor(() => expect(document.querySelector("[data-slot='read-only']")).not.toBeNull());
    expect(toolsButton()).toBeNull();
    expect(calls).not.toContain("GET /api/tool-servers");
  });
});
