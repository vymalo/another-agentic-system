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
async function makeThread(text: string, agent = "adam", untilStarted = false): Promise<string> {
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
const send = () => screen.getByRole("button", { name: "Send" });
const sentPosts = () => calls.filter((c) => c.startsWith("POST /agui/agents"));
/** The notes under the person's messages: how each was delivered, if it was sent while the agent worked. */
const notes = () =>
  [...log().querySelectorAll('[data-slot="delivery-note"]')].map((n) => n.textContent);

/** Lets a run the test holds (`gate`) go on. */
async function release(threadId: string) {
  const res = await realFetch(`${base}/__mock/release?thread=${threadId}`, { method: "POST" });
  expect(res.status).toBe(204);
}

describe("sending while the agent works, in the app", () => {
  it("Send: the message is read after the turn, and the bubble says it was sent while the agent worked", async () => {
    const id = await makeThread("gate refactor the parser", "adam", true);
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Working…"));
    // the replay is on screen before a send is offered
    await waitFor(() => expect(within(log()).getByText("gate refactor the parser")).toBeTruthy());
    type("echo you were wrong since line 1");
    await waitFor(() => expect((send() as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(send());
    await waitFor(() => expect(sentPosts()).toEqual(["POST /agui/agents/adam 200"]));
    // the message is on screen at once, once, with its note (the coder's card lists steer/v1)
    await waitFor(() =>
      expect(notes()).toEqual(["Sent while Adam was working · read at its next step"]),
    );
    expect(within(log()).getAllByText("echo you were wrong since line 1")).toHaveLength(1);
    // the first turn is still there, and the thread is still working: no turn reads "Stopped" (the
    // runtime's own send would have ended the first turn as cancelled)
    expect(within(log()).getAllByText("gate refactor the parser")).toHaveLength(1);
    expect(stateBadge().textContent).toBe("Working…");
    expect(log().textContent).not.toMatch(/Stopped ·|Stopped$/);
    expect(log().querySelectorAll('[data-slot="turn-summary-line"]').length).toBeGreaterThan(0);
    for (const line of log().querySelectorAll('[data-slot="turn-summary-text"]')) {
      expect(line.textContent).not.toMatch(/Stopped/);
    }
    expect(box().value).toBe("");

    // the agent finishes its turn and then reads the message: the next job answers it
    await release(id);
    await waitFor(() =>
      expect(
        within(log()).getAllByText(/echo: echo you were wrong since line 1/).length,
      ).toBeGreaterThan(0),
    );
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(notes()).toHaveLength(1);
    expect(within(log()).getAllByText("gate refactor the parser")).toHaveLength(1);
  });

  it("an agent whose card does not list steer/v1 says the message is read after its turn", async () => {
    const id = await makeThread("gate refactor the parser", "reviewer", true);
    shell(id);
    await waitFor(() => expect(within(log()).getByText("gate refactor the parser")).toBeTruthy());
    type("echo one more thing");
    await waitFor(() => expect((send() as HTMLButtonElement).disabled).toBe(false));
    expect(
      document.getElementById(send().getAttribute("aria-describedby") ?? "")?.textContent,
    ).toBe("Reviewer reads it after this turn");
    fireEvent.click(send());
    await waitFor(() =>
      expect(notes()).toEqual(["Sent while Reviewer was working · read after this turn"]),
    );
    await release(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
  });

  it("Stop and send, with the shortcut: the agent is stopped, the message starts the next job, and the bubble says so", async () => {
    const id = await makeThread("slow refactor the parser", "adam", true);
    shell(id);
    await waitFor(() => expect(within(log()).getByText("slow refactor the parser")).toBeTruthy());
    type("echo do X instead");
    await waitFor(() => expect((send() as HTMLButtonElement).disabled).toBe(false));
    fireEvent.keyDown(box(), { key: "Enter", ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(notes()).toEqual(["Stopped Adam · it starts again from here"]));
    // the thread never read done or cancelled for the abandoned job: it goes on to the next
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() =>
      expect(within(log()).getAllByText(/echo: echo do X instead/).length).toBeGreaterThan(0),
    );
    expect(within(log()).getAllByText("slow refactor the parser")).toHaveLength(1);
    expect(within(log()).getAllByText("echo do X instead")).toHaveLength(1);
    expect(sentPosts()).toEqual(["POST /agui/agents/adam 200"]);
  });

  it("a page opened after the message was sent shows the same note from the log", async () => {
    const id = await makeThread("gate refactor the parser", "adam", true);
    shell(id);
    await waitFor(() => expect(within(log()).getByText("gate refactor the parser")).toBeTruthy());
    type("echo you were wrong since line 1");
    await waitFor(() => expect((send() as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(send());
    await waitFor(() => expect(notes()).toHaveLength(1));
    await release(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    // another tab: the replay carries the delivery in the message's metadata
    cleanup();
    shell(id);
    await waitFor(() =>
      expect(notes()).toEqual(["Sent while Adam was working · read at its next step"]),
    );
    expect(within(log()).getAllByText("gate refactor the parser")).toHaveLength(1);
    expect(within(log()).getAllByText("echo you were wrong since line 1")).toHaveLength(1);
  });

  it("holds the send back until the conversation is on screen, so it cannot replace the turns it has not drawn", async () => {
    const id = await makeThread("gate refactor the parser", "adam", true);
    let letGo: () => void = () => {};
    holding = {
      key: `GET /agui/threads/${id}/connect`,
      until: new Promise<void>((r) => {
        letGo = r;
      }),
    };
    shell(id);
    // the state is known (the resource), the replay is not here yet
    await waitFor(() => expect(stateBadge().textContent).toBe("Working…"));
    type("echo too early");
    expect((send() as HTMLButtonElement).disabled).toBe(true);
    fireEvent.keyDown(box(), { key: "Enter" });
    await new Promise((r) => setTimeout(r, 50));
    expect(sentPosts()).toEqual([]);
    expect(box().value).toBe("echo too early");
    holding = undefined;
    letGo();
    await waitFor(() => expect(within(log()).getByText("gate refactor the parser")).toBeTruthy());
    await waitFor(() => expect((send() as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(send());
    await waitFor(() => expect(notes()).toHaveLength(1));
    // the first turn was not replaced
    expect(within(log()).getAllByText("gate refactor the parser")).toHaveLength(1);
    await release(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
  });

  it("an idle thread takes a message the ordinary way: no note, no split menu", async () => {
    const id = await makeThread("echo hi");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(within(log()).getByText("echo hi")).toBeTruthy());
    type("echo and again");
    expect(screen.queryByRole("button", { name: "Delivery options" })).toBeNull();
    fireEvent.click(send());
    await waitFor(() =>
      expect(within(log()).getAllByText(/echo: echo and again/).length).toBeGreaterThan(0),
    );
    expect(notes()).toEqual([]);
  });
});
