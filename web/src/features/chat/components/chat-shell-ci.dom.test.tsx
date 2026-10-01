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
 * CI results (ADR 0017) through the whole app: the golden `ci` played by the mock, read by
 * ThreadAgent, drawn by the runtime and the renderers. The mock plays a step every 100 ms, slow
 * enough for a test to see each state of the thread go by.
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
const reports = () => screen.queryAllByRole("listitem", { name: /^CI: / });
const checks = () => screen.queryAllByRole("listitem", { name: /^Check: / });
const dividers = () => [...document.querySelectorAll("[data-slot='rework-step']")];
/** The words of a rework step ("Checks failed — trying again (2/3)"), without its count. */
const lineOf = (step: Element) =>
  step.querySelector(":scope > div > div:first-child > span:first-child")?.textContent;

describe("CI results, in the app", () => {
  it("ci: a red report sends the agent back, a green one finishes the job; every report is a card", async () => {
    const id = await makeThread("verify-ci fix the login");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(reports()).toHaveLength(2));

    const [red, green] = reports() as [HTMLElement, HTMLElement];
    expect(red.getAttribute("aria-label")).toBe("CI: ci/build, failure");
    expect(green.getAttribute("aria-label")).toBe("CI: ci/build, success");
    expect(within(red).getByText("Failure")).toBeTruthy();
    expect(within(red).getByText("ci/build")).toBeTruthy();
    expect(within(red).getByText("1 test failed: tests::login")).toBeTruthy();
    expect(within(red).getByText("0000000").getAttribute("title")).toMatch(/0001$/);
    expect(within(red).getByText("agent/fix")).toBeTruthy();
    expect(within(red).getByText("Generic webhook · github.com/acme/demo")).toBeTruthy();
    const link = within(red).getByRole("link", { name: /^View run/ });
    expect(link.getAttribute("href")).toBe("https://ci.example.com/runs/1");
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer");
    expect(within(green).getByText("Success")).toBeTruthy();
    expect(within(green).getByText("3 tests passed")).toBeTruthy();
    expect(within(green).getByText("0000000").getAttribute("title")).toMatch(/0002$/);
    expect(
      within(green)
        .getByRole("link", { name: /^View run/ })
        .getAttribute("href"),
    ).toBe("https://ci.example.com/runs/2");

    // the gate's own cards: the pending check was replaced by the answer, one per attempt
    expect(checks().map((c) => c.getAttribute("aria-label"))).toEqual([
      "Check: CI, attempt 1, failed",
      "Check: CI, attempt 2, passed",
    ]);
    expect(dividers().map(lineOf)).toEqual(["CI failed — trying again (2/3)"]);
    const order = [
      ...log().querySelectorAll(
        "[data-slot='check-card'], [data-slot='ci-card'], [data-slot='rework-step']",
      ),
    ].map((n) => n.getAttribute("data-slot"));
    expect(order).toEqual(["check-card", "ci-card", "rework-step", "check-card", "ci-card"]);
    expect(screen.queryByText("Pending")).toBeNull();
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).disabled).toBe(false);
  });

  it("a fresh page shows the same cards, none doubled", async () => {
    const id = await makeThread("verify-ci fix the login");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(reports()).toHaveLength(2));
    const before = reports().map((c) => c.getAttribute("aria-label"));

    // another tab, opened now: the replay ends in the same place
    cleanup();
    shell(id);
    await waitFor(() => expect(reports()).toHaveLength(2));
    expect(stateBadge().textContent).toBe("Done");
    expect(reports().map((c) => c.getAttribute("aria-label"))).toEqual(before);
    expect(checks()).toHaveLength(2);
  });

  it("a reconnect after the first report shows the second one once, and the first as it was", async () => {
    const id = await makeThread("verify-ci fix the login");
    shell(id);
    await waitFor(() => expect(reports()).toHaveLength(1));
    // the network drops: the stream comes back from the last id
    expect((await realFetch(`${base}/__mock/drop-streams`, { method: "POST" })).status).toBe(204);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(reports()).toHaveLength(2));
    expect(reports().map((c) => c.getAttribute("data-conclusion"))).toEqual(["failure", "success"]);
    expect(checks()).toHaveLength(2);
    expect(dividers()).toHaveLength(1);
  });

  it("mid-run: the first report is on the page while the thread is still being verified", async () => {
    const id = await makeThread("verify-ci fix the login");
    shell(id);
    await waitFor(() => expect(reports()).toHaveLength(1));
    expect(reports()[0]?.getAttribute("data-conclusion")).toBe("failure");
    await waitFor(() => expect(reports()).toHaveLength(2));
    // the first one is still the first, and unchanged
    expect(reports()[0]?.getAttribute("data-conclusion")).toBe("failure");
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
  });

  it("verify-ci-stale (mock only): the late report of an older push has its own card next to the current one", async () => {
    const id = await makeThread("verify-ci-stale ship it");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(reports()).toHaveLength(2));
    expect(reports().map((c) => c.getAttribute("data-conclusion"))).toEqual(["failure", "success"]);
    expect(within(reports()[1] as HTMLElement).getByText("build passed")).toBeTruthy();
  });

  it("a thread that waits for CI has no report card yet", async () => {
    const id = await makeThread("verify-wait ship it");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Checking the work…"));
    await waitFor(() => expect(checks()).toHaveLength(2));
    expect(reports()).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Stopped"));
  });

  it("a thread without a gate has no report card", async () => {
    const id = await makeThread("echo hi");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(reports()).toHaveLength(0);
  });
});
