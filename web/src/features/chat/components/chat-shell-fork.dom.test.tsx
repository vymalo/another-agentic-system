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
 * Forking a chat (ADR 0029) through the whole app against the mock orchestrator: "Fork from here"
 * under a finished agent turn, disabled while the turn runs, the marker as a divider in the fork,
 * and the fork in the thread list.
 */

const push = vi.fn();
const router = { push }; // stable, like the App Router's
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

let ChatShell: typeof import("./chat-shell").ChatShell;

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** Every request the app made: `METHOD path status`. */
let calls: string[] = [];
/** Answer this request (by `METHOD path`) with a problem instead of asking the mock. */
let failing: { key: string; status: number; body: Record<string, unknown> } | undefined;

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
  // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
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
    const key = `${method} ${url.pathname}`;
    if (failing && key === failing.key) {
      calls.push(`${key} ${failing.status}`);
      return new Response(
        JSON.stringify({ title: "Failure", status: failing.status, ...failing.body }),
        {
          status: failing.status,
          headers: { "content-type": "application/problem+json" },
        },
      );
    }
    const res = await realFetch(abs(input) as RequestInfo, init);
    calls.push(`${key} ${res.status}`);
    return res;
  }) as typeof fetch;
  ({ ChatShell } = await import("./chat-shell"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});
beforeEach(() => {
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  window.localStorage.clear();
  calls = [];
  failing = undefined;
  push.mockClear();
});
afterEach(cleanup);

const shell = (threadId: string) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

async function makeThread(text: string, agent = "coder", untilStarted = false): Promise<string> {
  const threadId = uuidv7();
  const res = await realFetch(`${base}/agui/agents/${agent}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({
      threadId,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: text }],
    }),
  });
  if (untilStarted) await res.body?.cancel();
  else await res.text();
  return threadId;
}

/** The members of a thread (or an export) these tests read. */
type ApiJson = {
  id: string;
  lastSeq: number;
  state: string;
  target: unknown;
  forkedFrom?: { threadId: string; seq: number; kind: string };
  events: { seq: number; kind: string }[];
};

const api = async (path: string, init?: RequestInit) => {
  const res = await realFetch(`${base}${path}`, {
    headers: { "Content-Type": "application/json" },
    ...init,
  });
  return { status: res.status, body: (await res.json()) as ApiJson };
};

const log = () => screen.getByRole("log", { name: "Conversation" });
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
const forkButtons = () => within(log()).queryAllByRole("button", { name: "Fork from here" });

describe("fork from here, in the app", () => {
  it("a finished turn offers it; it makes a thread that holds the conversation up to the turn's end and goes there", async () => {
    const id = await makeThread("echo first");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(forkButtons()).toHaveLength(1));
    const button = forkButtons()[0] as HTMLElement;
    expect(button.getAttribute("aria-disabled")).toBe("false");
    fireEvent.click(button);

    await waitFor(() => expect(push).toHaveBeenCalledTimes(1));
    const forkId = String(push.mock.calls[0]?.[0]).replace("/threads/", "");
    expect(calls).toContain(`POST /api/threads/${id}/fork 201`);
    const { body: made } = await api(`/api/threads/${forkId}`);
    const { body: parent } = await api(`/api/threads/${id}`);
    expect(made.forkedFrom).toEqual({ threadId: id, seq: parent.lastSeq, kind: "fork" });
    expect(made.state).toBe("done");
    expect(made.target).toEqual(parent.target);
  });

  it("a turn that is going on cannot be forked: the button says so, and nothing is sent", async () => {
    const id = await makeThread("Refactor the module", "coder", true);
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Working…"));
    await waitFor(() => expect(forkButtons()).toHaveLength(1));
    const button = forkButtons()[0] as HTMLElement;
    expect(button.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(button);
    expect(calls.some((c) => c.includes("/fork"))).toBe(false);
    expect(push).not.toHaveBeenCalled();
    // the agent menu does not offer another agent either, and says why
    const trigger = await screen.findByRole("button", { name: /^Agent:/ });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    const menu = await screen.findByRole("menu");
    expect(
      within(menu)
        .getByRole("menuitemradio", { name: /^Reviewer/ })
        .getAttribute("aria-disabled"),
    ).toBe("true");
    expect(within(menu).getByText(/The agent is working/)).toBeTruthy();
  });

  it("once the turn is over (stopped) it can be forked", async () => {
    const id = await makeThread("Refactor the module", "coder", true);
    shell(id);
    await waitFor(() => expect(forkButtons()).toHaveLength(1));
    expect((forkButtons()[0] as HTMLElement).getAttribute("aria-disabled")).toBe("true");
    await realFetch(`${base}/api/threads/${id}/cancel`, { method: "POST" });
    await waitFor(() => expect(stateBadge().textContent).toBe("Stopped"));
    await waitFor(() =>
      expect((forkButtons()[0] as HTMLElement).getAttribute("aria-disabled")).toBe("false"),
    );
  });

  it("an earlier turn can be forked while the next one runs, and each turn has its own", async () => {
    const id = await makeThread("echo first");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(forkButtons()).toHaveLength(1));
    // a second job that does not end
    const box = screen.getByLabelText("Message");
    fireEvent.change(box, { target: { value: "Refactor the rest" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Working…"));
    await waitFor(() => expect(forkButtons()).toHaveLength(2));
    const [first, second] = forkButtons() as [HTMLElement, HTMLElement];
    expect(first.getAttribute("aria-disabled")).toBe("false");
    expect(second.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(first);
    await waitFor(() => expect(push).toHaveBeenCalledTimes(1));
    const forkId = String(push.mock.calls[0]?.[0]).replace("/threads/", "");
    // the copy ends before the second message
    const { body: made } = await api(`/api/threads/${forkId}`);
    const { body: exported } = await api(`/api/threads/${id}/export`);
    const second_message = (exported.events as { seq: number; kind: string }[]).filter(
      (e) => e.kind === "user_message",
    )[1];
    expect(made.forkedFrom?.seq).toBe((second_message?.seq ?? 0) - 1);
  });

  it("says why when the server refuses (the turn began after the page last heard) and keeps the page", async () => {
    const id = await makeThread("echo first");
    shell(id);
    await waitFor(() => expect(forkButtons()).toHaveLength(1));
    failing = { key: `POST /api/threads/${id}/fork`, status: 409, body: { code: "turn_open" } };
    fireEvent.click(forkButtons()[0] as HTMLElement);
    const alert = await screen.findByText(/The agent is still working on this turn/);
    expect(alert.closest('[role="alert"]')).toBeTruthy();
    expect(push).not.toHaveBeenCalled();
    // dismissed, and the page is as it was
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(screen.queryByText(/still working on this turn/)).toBeNull());
  });

  it("shows another problem's own words", async () => {
    const id = await makeThread("echo first");
    shell(id);
    await waitFor(() => expect(forkButtons()).toHaveLength(1));
    failing = {
      key: `POST /api/threads/${id}/fork`,
      status: 503,
      body: { detail: "storage is unavailable" },
    };
    fireEvent.click(forkButtons()[0] as HTMLElement);
    expect(await screen.findByText(/storage is unavailable/)).toBeTruthy();
  });
});

describe("a fork, as its own chat", () => {
  it("opens with the parent's conversation, then a divider that links to the parent", async () => {
    const id = await makeThread("echo first");
    const { body: parent } = await api(`/api/threads/${id}`);
    const { body: fork } = await api(`/api/threads/${id}/fork`, {
      method: "POST",
      body: JSON.stringify({ after: parent.lastSeq }),
    });
    shell(fork.id);
    await waitFor(() => expect(log().textContent).toContain("Forked from"));
    expect(log().textContent).toContain("echo first");
    const link = within(log()).getByRole("link", { name: "echo first" });
    expect(link.getAttribute("href")).toBe(`/threads/${id}`);
    // the same agent: it is not said to have changed
    expect(log().textContent).not.toContain("continued with");
    expect(stateBadge().textContent).toBe("Done");
    // a message goes on in the fork, and starts the next job
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "echo second" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(log().textContent).toContain("echo: echo second"));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
  });

  it("says which agent the conversation was continued with", async () => {
    const id = await makeThread("echo first");
    const { body: parent } = await api(`/api/threads/${id}`);
    const { body: fork } = await api(`/api/threads/${id}/fork`, {
      method: "POST",
      body: JSON.stringify({ after: parent.lastSeq, target: { agentId: "reviewer" } }),
    });
    shell(fork.id);
    await waitFor(() =>
      expect(log().textContent).toMatch(/Forked from .* · continued with reviewer/),
    );
    // the fork is the reviewer's
    expect((await screen.findByRole("button", { name: /^Agent:/ })).textContent).toContain(
      "Reviewer",
    );
  });

  it("a fork of a thread that waited for an answer takes an ordinary message", async () => {
    const id = await makeThread("ask which branch");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Your turn"));
    cleanup();
    const { body: fork } = await api(`/api/threads/${id}/fork`, {
      method: "POST",
      body: JSON.stringify({ after: 1 }),
    });
    shell(fork.id);
    await waitFor(() => expect(log().textContent).toContain("Forked from"));
    expect(stateBadge().textContent).toBe("Done");
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "echo go on" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(log().textContent).toContain("echo: echo go on"));
  });

  it("is a row of the thread list with a mark, and an edit of a message is not", async () => {
    const id = await makeThread("echo first");
    const { body: parent } = await api(`/api/threads/${id}`);
    const { body: fork } = await api(`/api/threads/${id}/fork`, {
      method: "POST",
      body: JSON.stringify({ after: parent.lastSeq }),
    });
    const { body: edit } = await api(`/api/threads/${id}/fork`, {
      method: "POST",
      body: JSON.stringify({ replace: 1, text: "echo other" }),
    });
    shell(id);
    const list = await screen.findByRole("navigation", { name: "Threads" });
    await waitFor(() => expect(within(list).getAllByRole("listitem").length).toBeGreaterThan(0));
    const rows = within(list).getAllByRole("link");
    const forkRow = rows.find((r) => r.getAttribute("href") === `/threads/${fork.id}`);
    expect(forkRow?.textContent).toContain("(fork)");
    expect(
      rows.find((r) => r.getAttribute("href") === `/threads/${id}`)?.textContent,
    ).not.toContain("(fork)");
    expect(rows.find((r) => r.getAttribute("href") === `/threads/${edit.id}`)).toBeUndefined();
  });
});
