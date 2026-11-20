// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import { cleanup, configure, fireEvent, render, screen, waitFor } from "@testing-library/react";
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

const server = createMockServer({ stepMs: 100, keepaliveMs: 1000, refreshMs: 400 });
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

const stateOf = async (id: string) =>
  ((await (await realFetch(`${base}/api/threads/${id}`)).json()) as { state: string }).state;
const settled = (id: string) =>
  waitFor(async () => expect(await stateOf(id)).toBe("blocked"), { timeout: 30_000 });
const plans = () => screen.queryAllByRole("region", { name: "Plan from reviewer" });
const said = () =>
  [...document.querySelectorAll('[data-slot="plan-status"] p')].map((p) => p.textContent);

// Two threads whose plans have the same surfaceId (and the same activity key: the mock numbers a thread's events from 1).
// What the page remembers of a plan is keyed by the thread, which `ChatShell` hands over in `ThreadView`: without it
// the first copy of the second thread is "a change" from the first thread's.
describe("a plan across threads, in the app", { timeout: 90_000 }, () => {
  it("the first copy of another thread's plan is not announced, its first update is, once", async () => {
    const a = await makeThread("plan now");
    const b = await makeThread("plan-rewrite now");
    await settled(a);
    await settled(b);

    const view = shell(a);
    await waitFor(() => expect(plans()).toHaveLength(1));
    expect(plans()[0]?.textContent).toContain("1 of 4 done");

    // the plan has been on the page for a while (longer than the announcer takes to settle)
    await new Promise((r) => setTimeout(r, 1300));
    // the person moves to the other thread: the same page, another thread
    view.rerender(
      <TooltipProvider>
        <ChatShell threadId={b} />
      </TooltipProvider>,
    );
    await waitFor(() => expect(plans()[0]?.textContent).toContain("ORIGINAL: delete the cache"));
    expect(plans()).toHaveLength(1);
    // quiet for longer than the announcer waits: a plan just met says nothing
    await new Promise((r) => setTimeout(r, 1800));
    expect(said()).toEqual([]);

    // the person answers, and the plan is sent again: that is announced, once
    const box = screen.getByRole("textbox");
    fireEvent.change(box, { target: { value: "ok go ahead" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(plans()).toHaveLength(2), { timeout: 30_000 });
    await waitFor(() => expect(said()).toEqual(["Plan updated: 0 of 2 done"]), { timeout: 30_000 });
    await new Promise((r) => setTimeout(r, 1500));
    expect(said()).toHaveLength(1);
  });
});
