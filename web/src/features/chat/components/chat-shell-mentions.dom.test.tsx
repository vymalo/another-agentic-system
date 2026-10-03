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

const push = vi.fn();
const router = { push }; // stable, like the App Router's
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

// The app in jsdom against the mock orchestrator: the shell, the runtime, the agent and LiveRuns.
let ChatShell: typeof import("./chat-shell").ChatShell;

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** Every request the app made: `METHOD path status`. */
let calls: string[] = [];
/** Answer this request (by `METHOD path`) with a problem instead of asking the mock. */
let failing: { key: string; status: number; detail: string } | undefined;
/** Hand the app this request's response (by `METHOD path`) only once `until` resolves. */
let holding: { key: string; until: Promise<void> } | undefined;

beforeAll(async () => {
  configure({ asyncUtilTimeout: 10_000 }); // the app and a mock server share the cores with other suites
  // jsdom has no layout: the viewport only needs these to exist
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollTo = () => {};
  Element.prototype.scrollIntoView = () => {};

  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  // the app calls relative URLs (the page's own origin); Node's Request wants absolute ones
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
        JSON.stringify({ title: "Failure", status: failing.status, detail: failing.detail }),
        { status: failing.status, headers: { "content-type": "application/problem+json" } },
      );
    }
    const res = await realFetch(abs(input) as RequestInfo, init);
    calls.push(`${key} ${res.status}`);
    if (holding && key === holding.key) await holding.until;
    return res;
  }) as typeof fetch;
  // the API client binds fetch and Request when it is created: import the app after the patch
  ({ ChatShell } = await import("./chat-shell"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});
beforeEach(() => {
  // a wide window: the panel is docked and open, which is where a thread's steps are listed
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  window.localStorage.clear();
  calls = [];
  failing = undefined;
  holding = undefined;
  push.mockClear();
});
afterEach(cleanup);

const shell = (threadId: string | null) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

/** A thread made the way any AG-UI client makes one; the promise ends with the run (or at RUN_STARTED). */
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

const log = () => screen.getByRole("log", { name: "Conversation" });
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
const box = () => screen.getByLabelText("Message") as HTMLTextAreaElement;
const type = (text: string) => fireEvent.change(box(), { target: { value: text } });
const sentPosts = () => calls.filter((c) => c.startsWith("POST /agui/agents"));

const key = (k: string, init: Record<string, unknown> = {}) =>
  fireEvent.keyDown(box(), { key: k, ...init });
const list = () => screen.queryByRole("listbox", { name: "Agents to mention" });
const optionIds = () =>
  list()
    ? within(list() as HTMLElement)
        .getAllByRole("option")
        .map((o) => o.getAttribute("data-agent"))
    : [];
const warning = () => document.querySelector('[data-slot="mentions-warning"]')?.textContent ?? null;
const chipsInLog = () =>
  [...log().querySelectorAll('[data-slot="mention"]')].map((c) => c.textContent);

/** A thread of `agent` that is done, drawn, the box ready for a follow-up. */
async function opened(agent: string) {
  const id = await makeThread("echo hi", agent);
  shell(id);
  await waitFor(() => expect(within(log()).getByText("echo hi")).toBeTruthy());
  await waitFor(() => expect(stateBadge().textContent).toMatch(/Done/));
  return id;
}

describe("mentions in the app", () => {
  it("a new chat offers the other agents, not the one that reads the message", async () => {
    shell(null);
    await waitFor(() => expect(box()).toBeTruthy());
    // the first agent (the coder) is the addressed one until another is picked
    await waitFor(() => expect(screen.getByRole("button", { name: /Coder/ })).toBeTruthy());
    type("@");
    await waitFor(() => expect(optionIds()).toEqual(["reviewer", "verifier"]));
  });

  it("a thread offers every agent but its own", async () => {
    await opened("reviewer");
    type("ask @");
    expect(optionIds()).toEqual(["coder", "verifier"]);
  });

  it("an agent that lists mentions/v1 and thread-tools/v1 is not warned about", async () => {
    await opened("coder");
    type("ask @rev");
    key("Enter");
    expect(box().value).toBe("ask @reviewer ");
    await waitFor(() => expect(capabilityRead("coder")).toBe(true));
    expect(warning()).toBeNull();
  });

  it("an agent that lists neither is told it will not be told", async () => {
    await opened("reviewer");
    type("ask @cod");
    key("Enter");
    await waitFor(() =>
      expect(warning()).toBe(
        "Reviewer does not use mentions, so it will not be told who you mentioned. The names stay in your message as text.",
      ),
    );
    // taking the mention off takes the line off
    fireEvent.click(screen.getByRole("button", { name: "Remove the mention of Coder" }));
    await waitFor(() => expect(warning()).toBeNull());
  });

  it("an agent that lists mentions/v1 but not thread-tools/v1 can be told, and cannot ask", async () => {
    await opened("verifier");
    type("ask @cod");
    key("Enter");
    await waitFor(() =>
      expect(warning()).toBe(
        "Verifier will be told who you mentioned, but it cannot ask other agents.",
      ),
    );
  });

  it("a card that cannot be read says so, and never says it can", async () => {
    failing = {
      key: "GET /agui/agents/reviewer/capabilities",
      status: 500,
      detail: "the card could not be read",
    };
    await opened("reviewer");
    type("ask @cod");
    key("Enter");
    await waitFor(() =>
      expect(warning()).toBe(
        "Could not check whether Reviewer can work with the agents you mentioned.",
      ),
    );
  });

  it("nothing is said while nobody is mentioned", async () => {
    await opened("reviewer");
    type("a plain follow-up");
    await waitFor(() => expect(capabilityRead("reviewer")).toBe(true));
    expect(warning()).toBeNull();
  });

  it("the sent message shows its mentions as chips, and a page opened later reads them from the log", async () => {
    const id = await opened("reviewer");
    type("😄 ask @cod");
    key("Enter");
    key("Enter"); // sends
    await waitFor(() => expect(sentPosts()).toHaveLength(1));
    await waitFor(() => expect(chipsInLog()).toEqual(["@coder"]));
    await waitFor(() => expect(box().value).toBe(""));
    cleanup();
    shell(id);
    await waitFor(() => expect(chipsInLog()).toEqual(["@coder"]));
    // the words around the chip are the person's, once
    expect(within(log()).getAllByText(/ask/)).toHaveLength(1);
  });

  it("a refused send shows the orchestrator's words and keeps the text with its mentions", async () => {
    await realFetch(`${base}/__mock/registry/agents`, {
      method: "POST",
      body: JSON.stringify({ id: "helper", name: "Helper" }),
    });
    try {
      await opened("reviewer");
      await waitFor(() => expect(calls.some((c) => c.startsWith("GET /api/agents"))).toBe(true));
      type("ask @hel");
      await waitFor(() => expect(optionIds()).toEqual(["helper"]));
      key("Enter");
      expect(box().value).toBe("ask @helper ");
      // the registry stops answering after the list was read
      await realFetch(`${base}/__mock/registry?down=true`, { method: "POST" });
      const reads = calls.filter((c) => c.startsWith("GET /api/agents")).length;
      key("Enter");
      const alert = await screen.findByText(/the agent registry could not answer/);
      expect(alert.closest('[role="alert"]')).toBeTruthy();
      await waitFor(() => expect(box().value).toBe("ask @helper "));
      await waitFor(() =>
        expect(document.querySelectorAll('[data-slot="mention-chip"]')).toHaveLength(1),
      );
      // nothing of it is in the conversation, and the agent list was read again
      expect(within(log()).queryByText(/ask/)).toBeNull();
      await waitFor(() =>
        expect(calls.filter((c) => c.startsWith("GET /api/agents")).length).toBeGreaterThan(reads),
      );
    } finally {
      await realFetch(`${base}/__mock/reset`, { method: "POST" });
    }
  });
});

/** Whether the capabilities of `agent` were read (the warning is judged only once they are). */
const capabilityRead = (agent: string) =>
  calls.some((c) => c.startsWith(`GET /agui/agents/${agent}/capabilities`));
