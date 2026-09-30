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
const counter = () => document.querySelector("[data-slot='attempt-counter']");
const cards = () => screen.queryAllByRole("region", { name: /^Check: / });
const dividers = () => [...document.querySelectorAll("[data-slot='rework-divider']")];

/** Every distinct text the badge and the counter show, in order, until the test ends. */
function recordHistory() {
  const badges: string[] = [];
  const counters: string[] = [];
  const take = () => {
    const badge = document.querySelector("[role='status'][aria-label^='Thread state:']");
    const text = badge?.textContent ?? "";
    if (text && badges.at(-1) !== text) badges.push(text);
    const attempt = counter()?.querySelector("[aria-hidden='true']")?.textContent ?? "";
    if (attempt && counters.at(-1) !== attempt) counters.push(attempt);
  };
  const observer = new MutationObserver(take);
  observer.observe(document.body, { childList: true, subtree: true, characterData: true });
  take();
  return { badges, counters, stop: () => observer.disconnect() };
}

/** `wanted` appears in `seen` in this order, with anything between. */
const inOrder = (seen: string[], wanted: string[]): boolean => {
  let i = 0;
  for (const s of seen) if (s === wanted[i]) i += 1;
  return i === wanted.length;
};

describe("a thread under the verification gate, in the app", () => {
  it("verify-green: verifying, sent back, working again, verifying, done; the counter goes 1/3 to 2/3", async () => {
    const id = await makeThread("verify-red-once fix the login");
    const seen = recordHistory();
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    seen.stop();

    expect(inOrder(seen.badges, ["Verifying", "Queued", "Working", "Verifying", "Done"])).toBe(
      true,
    );
    expect(seen.counters.at(-1)).toBe("Attempt 2/3");
    expect(inOrder(seen.counters, ["Attempt 1/3", "Attempt 2/3"])).toBe(true);
    // kept after the job is over: it is what the job took
    expect(counter()?.textContent).toContain("Attempt 2 of 3");

    // the cards, the divider and the second attempt, in the order the log has them
    await waitFor(() => expect(cards()).toHaveLength(2));
    const [failed, passed] = cards() as [HTMLElement, HTMLElement];
    expect(failed.getAttribute("data-status")).toBe("failed");
    expect(passed.getAttribute("data-status")).toBe("passed");
    expect(within(failed).getByText("tests::login fails: expected 200, got 500")).toBeTruthy();
    // the short commit (the first seven digits; the fake agent's commits differ further on)
    expect(within(failed).getByText("0000000").getAttribute("title")).toMatch(/0001$/);
    expect(within(passed).getByText("0000000").getAttribute("title")).toMatch(/0002$/);
    expect(dividers().map((d) => d.textContent)).toEqual([
      "Attempt 2 of 3: sent back with 1 finding",
    ]);
    const order = [
      ...log().querySelectorAll("[data-slot='check-card'], [data-slot='rework-divider']"),
    ];
    expect(order.map((n) => n.getAttribute("data-slot"))).toEqual([
      "check-card",
      "rework-divider",
      "check-card",
    ]);
    // a finished job is the ordinary finished thread
    expect(screen.getByText(/This thread is done\./)).toBeTruthy();
    expect(screen.queryByText(/Checks failed/)).toBeNull();
  });

  it("verify-red: out of attempts is 'Checks failed after 3 attempts', not an ordinary failure", async () => {
    const id = await makeThread("verify-red fix the login");
    const seen = recordHistory();
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Failed"));
    seen.stop();

    expect(
      inOrder(seen.badges, [
        "Verifying",
        "Queued",
        "Working",
        "Verifying",
        "Queued",
        "Working",
        "Verifying",
        "Failed",
      ]),
    ).toBe(true);
    expect(seen.counters.at(-1)).toBe("Attempt 3/3");
    await waitFor(() => expect(cards()).toHaveLength(3));
    expect(dividers().map((d) => d.textContent)).toEqual([
      "Attempt 2 of 3: sent back with 1 finding",
      "Attempt 3 of 3: sent back with 1 finding",
    ]);
    const notice = await screen.findByText("Checks failed after 3 attempts");
    expect(notice.closest("[data-slot='checks-failed']")).not.toBeNull();
    expect(screen.queryByText(/This thread is failed\./)).toBeNull();
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).disabled).toBe(true);
  });

  it("an agent failure is still an ordinary failure, with no counter", async () => {
    const id = await makeThread("fail please");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Failed"));
    await screen.findByText(/This thread is failed\./);
    expect(screen.queryByText(/Checks failed/)).toBeNull();
    expect(counter()).toBeNull();
  });

  it("a thread without a gate has no counter", async () => {
    const id = await makeThread("echo hi");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(counter()).toBeNull();
    expect(cards()).toHaveLength(0);
  });

  it("verify-ci: a pending card is replaced by its answer, a stale one stands apart", async () => {
    const id = await makeThread("verify-ci ship it");
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

  it("a reconnect mid-verification shows the same state; a fresh page shows it too; Cancel ends it", async () => {
    const id = await makeThread("verify-wait ship it");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Verifying"));
    await waitFor(() => expect(cards()).toHaveLength(2));
    const before = cards().map((c) => c.getAttribute("aria-label"));
    expect(before).toEqual([
      "Check: Agent checks, attempt 1, passed",
      "Check: CI, attempt 1, pending",
    ]);
    expect(counter()?.textContent).toContain("Attempt 1 of 3");
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).placeholder).toBe(
      "The work is being checked…",
    );

    // the network drops: the stream comes back from the last id and nothing is doubled
    expect((await realFetch(`${base}/__mock/drop-streams`, { method: "POST" })).status).toBe(204);
    await waitFor(() => expect(screen.queryByText("Reconnecting…")).not.toBeNull());
    await waitFor(() => expect(screen.queryByText("Reconnecting…")).toBeNull());
    expect(stateBadge().textContent).toBe("Verifying");
    expect(cards().map((c) => c.getAttribute("aria-label"))).toEqual(before);
    expect(counter()?.textContent).toContain("Attempt 1 of 3");

    // another tab, opened now: the replay ends in the same place
    cleanup();
    shell(id);
    await waitFor(() => expect(cards()).toHaveLength(2));
    expect(stateBadge().textContent).toBe("Verifying");
    expect(cards().map((c) => c.getAttribute("aria-label"))).toEqual(before);
    expect(counter()?.textContent).toContain("Attempt 1 of 3");
    expect(document.querySelectorAll("[data-slot='rework-divider']")).toHaveLength(0);

    // Cancel is offered while the work is verified
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Cancelled"));
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
