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
 * Editing a message into a branch (ADR 0029) through the whole app against the mock orchestrator:
 * the editor in the bubble, the new thread it goes to (scrolled to its message), the `‹ n/m ›` of a
 * message that has other versions, and the thread list that shows a conversation once.
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
const bubbles = () => Array.from(log().querySelectorAll<HTMLElement>('[data-slot="user-message"]'));
const editButtons = () => within(log()).queryAllByRole("button", { name: "Edit what you said" });
const pickers = () =>
  Array.from(log().querySelectorAll<HTMLElement>('[data-slot="branch-picker"]'));

/** A finished thread of two jobs and the same thread edited at its second message. */
async function editedPair() {
  const id = await makeThread("echo first");
  const send = await realFetch(`${base}/agui/agents/coder`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({
      threadId: id,
      runId: "run-2",
      messages: [{ id: "m-2", role: "user", content: "echo second" }],
    }),
  });
  await send.text();
  const { body: exported } = await api(`/api/threads/${id}/export`);
  const messages = exported.events.filter((e) => e.kind === "user_message");
  const second = messages[1]?.seq ?? 0;
  const { body: edit } = await api(`/api/threads/${id}/fork`, {
    method: "POST",
    body: JSON.stringify({ replace: second, text: "echo other" }),
  });
  return { id, edit: edit.id, first: messages[0]?.seq ?? 0, second };
}

describe("a message of the person, edited", () => {
  it("has an Edit button once its place in the log is known; the editor takes the bubble's place, Escape gives it back", async () => {
    const id = await makeThread("echo first");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(editButtons()).toHaveLength(1));
    // the bubble names its event
    expect(bubbles()[0]?.getAttribute("id")).toBe("m-1");
    expect(bubbles()[0]?.getAttribute("data-seq")).toBe("1");
    const button = editButtons()[0] as HTMLElement;
    fireEvent.click(button);
    const field = (await screen.findByRole("textbox", {
      name: "What you said",
    })) as HTMLTextAreaElement;
    expect(field.value).toBe("echo first");
    expect(document.activeElement).toBe(field);
    fireEvent.keyDown(field, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("textbox", { name: "What you said" })).toBeNull(),
    );
    expect(bubbles()[0]?.textContent).toContain("echo first");
    expect(push).not.toHaveBeenCalled();
    expect(calls.some((c) => c.includes("/fork"))).toBe(false);
  });

  it("Ctrl+Enter sends it: a new thread holds the messages before it and the new words, and the page goes there, to the new message", async () => {
    const id = await makeThread("echo first");
    shell(id);
    await waitFor(() => expect(editButtons()).toHaveLength(1));
    fireEvent.click(editButtons()[0] as HTMLElement);
    const field = await screen.findByRole("textbox", { name: "What you said" });
    fireEvent.change(field, { target: { value: "echo changed" } });
    fireEvent.keyDown(field, { key: "Enter", ctrlKey: true });

    await waitFor(() => expect(push).toHaveBeenCalledTimes(1));
    const [to] = String(push.mock.calls[0]?.[0]).split("#");
    expect(String(push.mock.calls[0]?.[0])).toMatch(/^\/threads\/[0-9a-f-]{36}#m-2$/);
    const forkId = String(to).replace("/threads/", "");
    const { body: made } = await api(`/api/threads/${forkId}`);
    expect(made.forkedFrom).toEqual({ threadId: id, seq: 0, kind: "edit" });
    expect(calls).toContain(`POST /api/threads/${id}/fork 201`);
    // the old thread is as it was
    cleanup();
    shell(id);
    await waitFor(() => expect(log().textContent).toContain("echo: echo first"));
  });

  it("a refusal is said as a message about the edit, and the editor stays with the words", async () => {
    const id = await makeThread("echo first");
    shell(id);
    await waitFor(() => expect(editButtons()).toHaveLength(1));
    fireEvent.click(editButtons()[0] as HTMLElement);
    failing = {
      key: `POST /api/threads/${id}/fork`,
      status: 503,
      body: { detail: "storage is unavailable" },
    };
    const field = (await screen.findByRole("textbox", {
      name: "What you said",
    })) as HTMLTextAreaElement;
    fireEvent.change(field, { target: { value: "echo changed" } });
    fireEvent.keyDown(field, { key: "Enter", ctrlKey: true });
    const alert = await screen.findByText(/Could not edit the message: .*storage is unavailable/);
    expect(alert.closest('[role="alert"]')).toBeTruthy();
    expect(push).not.toHaveBeenCalled();
    expect(
      (screen.getByRole("textbox", { name: "What you said" }) as HTMLTextAreaElement).value,
    ).toBe("echo changed");
  });
});

describe("the versions of a message", () => {
  it("a message that was edited shows its place among the versions, and the picker goes to the neighbour", async () => {
    const { id, edit, second } = await editedPair();
    shell(edit);
    await waitFor(() => expect(log().textContent).toContain("echo: echo other"));
    await waitFor(() => expect(pickers()).toHaveLength(1));
    // the first message is the parent's own and has no other version
    expect(bubbles()).toHaveLength(2);
    const [picker] = pickers() as [HTMLElement];
    expect(bubbles()[1]?.contains(picker)).toBe(true);
    expect(picker.textContent).toContain("2/2");
    expect(within(picker).getByRole("status").textContent).toBe("Version 2 of 2");
    expect(
      (within(picker).getByRole("button", { name: "Next version" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    fireEvent.click(within(picker).getByRole("button", { name: "Previous version" }));
    expect(push).toHaveBeenCalledWith(`/threads/${id}#m-${second}`);
  });

  it("the original shows 1/2 and goes on to the edit, at the edit's message", async () => {
    const { id, edit } = await editedPair();
    shell(id);
    await waitFor(() => expect(log().textContent).toContain("echo: echo second"));
    await waitFor(() => expect(pickers()).toHaveLength(1));
    const [picker] = pickers() as [HTMLElement];
    expect(picker.textContent).toContain("1/2");
    expect(
      (within(picker).getByRole("button", { name: "Previous version" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    fireEvent.click(within(picker).getByRole("button", { name: "Next version" }));
    const { body: made } = await api(`/api/threads/${edit}`);
    expect(made.forkedFrom?.kind).toBe("edit");
    expect(String(push.mock.calls[0]?.[0])).toMatch(new RegExp(`^/threads/${edit}#m-\\d+$`));
  });

  it("a thread that was never edited has no picker", async () => {
    const id = await makeThread("echo first");
    shell(id);
    await waitFor(() => expect(editButtons()).toHaveLength(1));
    expect(pickers()).toHaveLength(0);
  });

  it("a picker that cannot be read is only absent: the chat works", async () => {
    const { id } = await editedPair();
    failing = { key: `GET /api/threads/${id}/branches`, status: 500, body: {} };
    shell(id);
    await waitFor(() => expect(log().textContent).toContain("echo: echo second"));
    await waitFor(() => expect(calls).toContain(`GET /api/threads/${id}/branches 500`));
    expect(pickers()).toHaveLength(0);
    expect(editButtons().length).toBeGreaterThan(0);
  });
});

describe("the thread list and the edits", () => {
  it("shows a conversation once: the edit is not a row, and the root is the open chat while an edit is open", async () => {
    const { id, edit } = await editedPair();
    shell(edit);
    const list = await screen.findByRole("navigation", { name: "Threads" });
    await waitFor(() =>
      expect(
        within(list)
          .getAllByRole("link")
          .some((l) => l.getAttribute("href") === `/threads/${id}`),
      ).toBe(true),
    );
    const rows = within(list).getAllByRole("link");
    expect(rows.find((r) => r.getAttribute("href") === `/threads/${edit}`)).toBeUndefined();
    const root = rows.find((r) => r.getAttribute("href") === `/threads/${id}`) as HTMLElement;
    // not the address that is open, but the conversation that is
    await waitFor(() => expect(root.getAttribute("aria-current")).toBe("true"));
    expect(root.className).toContain("bg-sidebar-accent");
  });
});
