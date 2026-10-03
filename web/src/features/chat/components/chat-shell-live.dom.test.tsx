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
 * Live text (ADR 0027) through the whole app: the mock relays the pieces of a reply as the
 * orchestrator does (frames with `metadata["vymalo.live"]`, never in the log), `ThreadAgent` keeps
 * them as drafts out of the runtime, and the turn draws them; the log's message, when it comes, is
 * the one reply. The mock plays a step every 250 ms, slow enough to see the words grow (a viewer hears only
 * the pieces that come after it connects, so the app is warmed up before a thread starts).
 */

const router = { push: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

let ChatShell: typeof import("./chat-shell").ChatShell;

const server = createMockServer({ stepMs: 250, keepaliveMs: 1000, refreshMs: 400 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;

beforeAll(async () => {
  configure({ asyncUtilTimeout: 20_000 });
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
  globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) =>
    realFetch(abs(input) as RequestInfo, init)) as typeof fetch;
  ({ ChatShell } = await import("./chat-shell"));
  // the first render of the app is slow (lazy chunks, cold caches): do it before a reply starts
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  render(
    <TooltipProvider>
      <ChatShell threadId={uuidv7()} />
    </TooltipProvider>,
  );
  await screen.findByText(/Thread not found/);
  cleanup();
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
});
afterEach(cleanup);

const shell = (threadId: string) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

/** A thread made the way any AG-UI client makes one; returns once its run has started. */
async function makeThread(text: string, agent = "reviewer"): Promise<string> {
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
  await res.body?.cancel();
  return threadId;
}

/**
 * Lets a held reply (`stream-gate`, `stream-abandon`) go on. The mock says 409 until the reply has
 * come to the place where it waits, which is what this waits for.
 */
async function release(threadId: string) {
  await waitFor(async () => {
    const released = await realFetch(`${base}/__mock/release?thread=${threadId}`, {
      method: "POST",
    });
    expect(released.status).toBe(204);
  });
}

const log = () => screen.getByRole("log", { name: "Conversation" });
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
const drafts = () => [...log().querySelectorAll<HTMLElement>('[data-slot="agent-draft"]')];
const replies = () => [...log().querySelectorAll<HTMLElement>('[data-slot="agent-message"]')];
/** How many times `words` is in the conversation, drafts and replies alike. */
const times = (words: string) => (log().textContent ?? "").split(words).length - 1;

// Each wait may take as long as `asyncUtilTimeout` (20 s) under a loaded run, and a test makes several,
// so the test as a whole gets more than the config's 20 s.
describe("live text, in the app", { timeout: 60_000 }, () => {
  it("stream-gate: the words grow in the turn as a draft, then the log's message is the one reply and no draft is left", async () => {
    // the mock holds the reply after its fifth piece until `release`: what the test looks at is
    // there as long as it needs, whatever the machine's speed (a reply that plays on its own is
    // over in three seconds, which a loaded machine can spend before the page has drawn it)
    const id = await makeThread("stream-gate write the plan");
    shell(id);

    // a draft in the agent's turn, with the first words, busy and silent for a screen reader
    await waitFor(() => expect(drafts()).toHaveLength(1));
    const draft = drafts()[0] as HTMLElement;
    expect(draft.getAttribute("aria-busy")).toBe("true");
    expect(draft.getAttribute("aria-live")).toBe("off");
    expect(draft.closest('[data-slot="agent-turn"]')).not.toBeNull();
    // not the finished reply: that is the runtime's message, which the log does not have yet
    expect(replies()).toHaveLength(0);
    const first = draft.textContent ?? "";
    expect(first.startsWith("I'll start with the failing test")).toBe(true);

    // it grows
    await waitFor(() =>
      expect((drafts()[0]?.textContent ?? "").length).toBeGreaterThan(first.length),
    );
    await waitFor(() => expect(drafts()[0]?.textContent).toContain("that fixes it."));
    // Markdown as it is written: the list is a list
    await waitFor(() => expect(drafts()[0]?.querySelectorAll("li").length).toBeGreaterThan(0));
    expect(replies()).toHaveLength(0);
    await release(id);

    // the log says the reply: one message, the draft is gone, the words are there once
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(drafts()).toHaveLength(0));
    expect(replies()).toHaveLength(1);
    expect(replies()[0]?.textContent).toContain("when it is green.");
    expect(replies()[0]?.querySelectorAll("li")).toHaveLength(3);
    expect(times("I'll start with the failing test")).toBe(1);
    expect(times("when it is green.")).toBe(1);
  });

  it("stream: the golden's three pieces, one message at the end", async () => {
    const id = await makeThread("stream write fibonacci in rust");
    shell(id);
    await waitFor(() => expect(drafts()[0]?.textContent).toContain("Fib"));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(drafts()).toHaveLength(0));
    expect(replies().map((r) => r.textContent)).toEqual(["Fibonacci in Rust."]);
    expect(times("Fibonacci in Rust.")).toBe(1);
  });

  // ADR 0031: a live message opens before anyone knows what its words are for. The words before a
  // tool call turn out to be working text: the draft leaves the column and is a note in Activity.
  // The reply that ends the turn is the answer: its draft stays, and is the one message.
  it("stream-words: a draft that ends as working text leaves the column for Activity; the reply stays", async () => {
    const id = await makeThread("stream-words go");
    shell(id);
    await waitFor(() => expect(drafts()[0]?.textContent).toContain("Let me run"));
    const activity = () =>
      within(screen.getByRole("complementary", { name: "Thread details" })).getByRole("tabpanel", {
        name: "Activity",
      });
    // the log's message says it was working: no draft of it, no reply of it, and it is a note
    await waitFor(() =>
      expect(activity().querySelector('li[data-kind="note"]')?.textContent).toContain(
        "Let me run the tests first.",
      ),
    );
    // not in the column as words (while the turn runs its line shows it, as the ticker)
    expect(drafts().filter((d) => d.textContent?.includes("tests first"))).toHaveLength(0);
    expect(replies()).toHaveLength(0);
    expect(log().querySelector('[data-slot="turn-ticker"]')?.textContent).toBe(
      "Let me run the tests first.",
    );
    // the reply is written as a draft of the answer, and stays as the one reply
    await waitFor(() => expect(drafts()[0]?.textContent).toContain("Streaming a reply"));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(drafts()).toHaveLength(0));
    expect(replies().map((r) => r.textContent)).toEqual([
      "Streaming a reply, word by word, as it is written.",
    ]);
    await waitFor(() => expect(times("Let me run the tests first.")).toBe(0));
    expect(activity().querySelectorAll('li[data-kind="note"]')).toHaveLength(1);
  });

  it("a reply that is still being written is a draft the transcript does not hold, and Stop ends it", async () => {
    const id = await makeThread("stream-hold write the plan");
    shell(id);
    await waitFor(() =>
      expect(drafts()[0]?.textContent).toContain("then make the smallest change"),
    );
    expect(replies()).toHaveLength(0);
    // the turn says it works: the draft stands in place of the starting line
    expect(within(log()).queryByText(/is starting/)).toBeNull();

    fireEvent.click(await screen.findByRole("button", { name: "Stop" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Stopped"));
    // given up with the run: nothing is left on screen that looks alive
    await waitFor(() => expect(drafts()).toHaveLength(0));
    expect(times("then make the smallest change")).toBe(0);
  });

  it("a stream that is given up goes, and the words the agent says next are said once", async () => {
    const id = await makeThread("stream-abandon what is the answer");
    shell(id);
    await waitFor(() => expect(drafts()[0]?.textContent).toContain("The answer is forty-"));
    // the model stalls halfway, and fails when the test says so
    await release(id);
    await waitFor(() => expect(drafts()).toHaveLength(0));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(replies().map((r) => r.textContent)).toEqual([
      "Sorry, let me say that again: it is forty-two.",
    ]);
    expect(times("The answer is")).toBe(0);
    expect(times("it is forty-two.")).toBe(1);
  });

  it("a connection cut mid-reply: the new one has the log only, and the reply arrives once, whole", async () => {
    const id = await makeThread("stream-gate write the plan");
    shell(id);
    await waitFor(() => expect(drafts()).toHaveLength(1));
    // the network went away: every open stream is cut; the drafts go with the connection
    await realFetch(`${base}/__mock/drop-streams`, { method: "POST" });
    await waitFor(() => expect(drafts()).toHaveLength(0));
    // the reply goes on while the page is reconnecting, and is over when it is back
    await release(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(replies()).toHaveLength(1));
    expect(replies()[0]?.textContent).toContain("when it is green.");
    expect(drafts()).toHaveLength(0);
    expect(times("I'll start with the failing test")).toBe(1);
  });

  it("a page opened mid-reply is told the text so far by the sender's refresh, from the start, once", async () => {
    const id = await makeThread("stream-hold write the plan");
    shell(id);
    await waitFor(() =>
      expect(drafts()[0]?.textContent).toContain("then make the smallest change"),
    );
    cleanup(); // the tab is closed
    shell(id); // and opened again: its connection has heard none of the pieces
    await waitFor(() => expect(drafts()).toHaveLength(1));
    expect(drafts()[0]?.textContent).toContain("I'll start with the failing test");
    expect(times("I'll start with the failing test")).toBe(1);
    // the same words, not a second copy of them, as the refresh comes round again
    await new Promise((r) => setTimeout(r, 900));
    expect(drafts()).toHaveLength(1);
    expect(times("I'll start with the failing test")).toBe(1);
    fireEvent.click(await screen.findByRole("button", { name: "Stop" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Stopped"));
  });

  it("a thread opened after the reply read the plain message: no draft, once", async () => {
    const id = await makeThread("stream write fibonacci in rust");
    // let the run finish before anyone looks
    for (let i = 0; i < 100; i++) {
      const t = (await (await realFetch(`${base}/api/threads/${id}`)).json()) as { state: string };
      if (t.state === "done") break;
      await new Promise((r) => setTimeout(r, 50));
    }
    shell(id);
    await waitFor(() => expect(replies()).toHaveLength(1));
    expect(drafts()).toHaveLength(0);
    expect(times("Fibonacci in Rust.")).toBe(1);
  });
});
