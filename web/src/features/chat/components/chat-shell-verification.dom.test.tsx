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
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { uuidv7 } from "@/lib/uuid";
import { createMockServer } from "../../../../mock/server";

/*
 * The verification gate (ADR 0018) through the whole app: the goldens verify-green and verify-red
 * played by the mock, read by ThreadAgent, drawn by the runtime and the renderers. The mock plays a
 * step every 100 ms, slow enough for a test to see each state of the thread go by.
 */

const router = { push: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

let ChatShell: typeof import("./chat-shell").ChatShell;

const server = createMockServer({ stepMs: 100, keepaliveMs: 1000 });
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
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
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

const log = () => screen.getByRole("log", { name: "Conversation" });
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
const cards = () => screen.queryAllByRole("listitem", { name: /^Check: / });
const dividers = () => [...document.querySelectorAll("[data-slot='rework-step']")];
/** The one line of a rework step (its findings are folded under it). */
const lineOf = (step: Element) => step.querySelector(":scope > div > div:first-child")?.textContent;

/** Every distinct text the state pill shows, in order, until the test ends. */
function recordHistory() {
  const badges: string[] = [];
  const take = () => {
    const badge = document.querySelector("[role='status'][aria-label^='Thread state:']");
    const text = badge?.textContent ?? "";
    if (text && badges.at(-1) !== text) badges.push(text);
  };
  const observer = new MutationObserver(take);
  observer.observe(document.body, { childList: true, subtree: true, characterData: true });
  take();
  return { badges, stop: () => observer.disconnect() };
}

/** `wanted` appears in `seen` in this order, with anything between. */
const inOrder = (seen: string[], wanted: string[]): boolean => {
  let i = 0;
  for (const s of seen) if (s === wanted[i]) i += 1;
  return i === wanted.length;
};

describe("a thread under the verification gate, in the app", () => {
  it("verify-green: checking, sent back, working again, checking, done; the header has no attempt counter", async () => {
    const id = await makeThread("verify-red-once fix the login");
    const seen = recordHistory();
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    seen.stop();

    expect(
      inOrder(seen.badges, [
        "Checking the work…",
        "Starting…",
        "Working…",
        "Checking the work…",
        "Done",
      ]),
    ).toBe(true);
    // attempts show inside the turn, in the rework step, and nowhere in the header
    expect(document.querySelector("[data-slot='attempt-counter']")).toBeNull();

    // the cards, the rework and the second attempt, in the order the log has them
    await waitFor(() => expect(cards()).toHaveLength(2));
    const [failed, passed] = cards() as [HTMLElement, HTMLElement];
    expect(failed.getAttribute("data-status")).toBe("failed");
    expect(passed.getAttribute("data-status")).toBe("passed");
    expect(within(failed).getByText("tests::login fails: expected 200, got 500")).toBeTruthy();
    // the short commit (the first seven digits; the fake agent's commits differ further on)
    expect(within(failed).getByText("0000000").getAttribute("title")).toMatch(/0001$/);
    expect(within(passed).getByText("0000000").getAttribute("title")).toMatch(/0002$/);
    expect(dividers().map(lineOf)).toEqual(["Checks failed — trying again (2/3)"]);
    const order = [
      ...log().querySelectorAll("[data-slot='check-card'], [data-slot='rework-step']"),
    ];
    expect(order.map((n) => n.getAttribute("data-slot"))).toEqual([
      "check-card",
      "rework-step",
      "check-card",
    ]);
    // a finished job is not a closed thread: the box is open, and there is nothing to say about it
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).disabled).toBe(false);
    expect(screen.queryByText(/This thread is/)).toBeNull();
    // the attempt that failed is a rework step; the job did not fail its checks
    expect(screen.queryByText(/Checks failed after/)).toBeNull();
    expect(document.querySelector("[data-slot='checks-failed']")).toBeNull();
  });

  it("verify-red: out of attempts is 'Checks failed after 3 attempts', not an ordinary failure", async () => {
    const id = await makeThread("verify-red fix the login");
    const seen = recordHistory();
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Failed"));
    seen.stop();

    expect(
      inOrder(seen.badges, [
        "Checking the work…",
        "Starting…",
        "Working…",
        "Checking the work…",
        "Starting…",
        "Working…",
        "Checking the work…",
        "Failed",
      ]),
    ).toBe(true);
    await waitFor(() => expect(cards()).toHaveLength(3));
    expect(dividers().map(lineOf)).toEqual([
      "Checks failed — trying again (2/3)",
      "Checks failed — trying again (3/3)",
    ]);
    const notice = await screen.findByText("Checks failed after 3 attempts");
    expect(notice.closest("[data-slot='checks-failed']")).not.toBeNull();
    expect(screen.queryByText(/This thread is failed\./)).toBeNull();
    expect(screen.queryByText(/Start a new thread/)).toBeNull();
    // the thread is not locked: write to go on, tell the agent how
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    expect(box.disabled).toBe(false);
    expect(box.placeholder).toBe("Tell the agent how to go on…");
  });

  it("an agent failure is an ordinary failure, and the box stays open", async () => {
    const id = await makeThread("fail please");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Failed"));
    expect(screen.queryByText(/Checks failed/)).toBeNull();
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).disabled).toBe(false);
  });

  it("a thread without a gate has no cards", async () => {
    const id = await makeThread("echo hi");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(cards()).toHaveLength(0);
  });

  it("verify-ci-stale: a pending card is replaced by its answer, a stale one stands apart", async () => {
    const id = await makeThread("verify-ci-stale ship it");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(cards()).toHaveLength(3));
    // agent checks, then CI (was pending; replaced in place, not added), then the stale CI answer
    expect(cards().map((c) => c.getAttribute("aria-label"))).toEqual([
      "Check: Agent checks, attempt 1, passed",
      "Check: CI, attempt 1, passed",
      "Check: CI, attempt 1, failed, stale",
    ]);
    expect(within(cards()[1] as HTMLElement).getByText("build passed")).toBeTruthy();
    expect(within(cards()[2] as HTMLElement).getByText("Stale")).toBeTruthy();
    expect(screen.queryByText("Pending")).toBeNull();
  });

  it("a reconnect mid-verification shows the same state; a fresh page shows it too; Stop ends it", async () => {
    const id = await makeThread("verify-wait ship it");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Checking the work…"));
    await waitFor(() => expect(cards()).toHaveLength(2));
    const before = cards().map((c) => c.getAttribute("aria-label"));
    expect(before).toEqual([
      "Check: Agent checks, attempt 1, passed",
      "Check: CI, attempt 1, pending",
    ]);
    // the box is for drafting while the work is checked; Stop is what the button offers
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    expect(box.disabled).toBe(false);
    expect(box.placeholder).toBe("Send a follow-up…");

    // the network drops: the stream comes back from the last id and nothing is doubled
    expect((await realFetch(`${base}/__mock/drop-streams`, { method: "POST" })).status).toBe(204);
    await waitFor(() => expect(screen.queryByText("Reconnecting…")).not.toBeNull());
    await waitFor(() => expect(screen.queryByText("Reconnecting…")).toBeNull());
    expect(stateBadge().textContent).toBe("Checking the work…");
    expect(cards().map((c) => c.getAttribute("aria-label"))).toEqual(before);

    // another tab, opened now: the replay ends in the same place
    cleanup();
    shell(id);
    await waitFor(() => expect(cards()).toHaveLength(2));
    expect(stateBadge().textContent).toBe("Checking the work…");
    expect(cards().map((c) => c.getAttribute("aria-label"))).toEqual(before);
    expect(document.querySelectorAll("[data-slot='rework-step']")).toHaveLength(0);

    // Stop is offered while the work is verified
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Stopped"));
    expect(cards().map((c) => c.getAttribute("aria-label"))).toEqual(before);
  });

  it("the resource API tells the job before any stream does", async () => {
    const id = await makeThread("verify-red-once fix the login");
    const thread = (await (await realFetch(`${base}/api/threads/${id}`)).json()) as {
      job?: { attempt: number; maxAttempts: number; gate: string[] };
    };
    expect(thread.job).toMatchObject({ attempt: 1, maxAttempts: 3, gate: ["agent_checks"] });
    const plain = await makeThread("echo hi");
    expect(
      (await (await realFetch(`${base}/api/threads/${plain}`)).json()) as object,
    ).not.toHaveProperty("job");
  });
});
