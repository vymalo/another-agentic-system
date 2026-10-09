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
 * The answer in the chat, the working text in the panel (ADR 0031, plan 10 S6), through the whole
 * app against the mock orchestrator: the chat's column holds one answer per turn, the rest of what
 * the agent said while it worked is a note among the steps of the Activity tab, and a surface the
 * agent drew on the way stays in the column. The owner's own chat is `coder-notes`, in other words.
 */

const router = { push: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

let ChatShell: typeof import("./chat-shell").ChatShell;

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;

beforeAll(async () => {
  configure({ asyncUtilTimeout: 15_000 });
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
});
afterEach(cleanup);

const shell = (threadId: string) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

/** A thread made the way any AG-UI client makes one; resolves with the run (or at its start). */
async function makeThread(text: string, untilStarted = false): Promise<string> {
  const threadId = uuidv7();
  const res = await realFetch(`${base}/agui/agents/adam`, {
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
const activity = () =>
  within(screen.getByRole("complementary", { name: "Thread details" })).getByRole("tabpanel", {
    name: "Activity",
  });
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
/** What the agent's words are in the column: the answers, one per turn. */
const answers = () => [...log().querySelectorAll<HTMLElement>('[data-slot="agent-message"]')];
const turns = () => [...log().querySelectorAll<HTMLElement>('[data-slot="agent-turn"]')];
/** The rows of the panel's tree that are notes or tools, in order, as `kind: words`. */
const rows = () =>
  [...activity().querySelectorAll<HTMLElement>('li[data-kind="note"], li[data-kind="tool"]')].map(
    (li) =>
      li.dataset.kind === "note"
        ? `note: ${li.querySelector('[data-slot="note-text"]')?.textContent?.replace("Working note: ", "")}`
        : `tool: ${li.querySelector('[data-slot="step-toggle"]')?.textContent ?? li.textContent}`,
  );

const NOTES = [
  "I'll build something small in a scratch project first, then show you the export.",
  "Your screen draws text, cards and diagrams, not images. So Node draws it and I show its shapes.",
  "Now the tests for both.",
  "Node 24 wants an explicit glob for the test directory. I'll fix the script, not the tests.",
  "All 7 tests pass. Now I'll export the drawing.",
  "Here is the drawing. Your screen has no images, so these are its shapes.",
];
const ANSWER = "The drawing is exported, and its shape is in the cards above.";

/** The turn in the owner's chat: the notes in Activity, one answer and the surface in the column. */
async function expectTheOwnersChat() {
  await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
  // the answer itself, not the first sentence of an old log, which is a draft of the answer until a step follows it
  await waitFor(() => {
    expect(answers()).toHaveLength(1);
    expect(answers()[0]?.textContent).toContain(ANSWER);
  });
  // one answer, and none of the sentences said while the agent worked, not even folded
  const column = log().textContent ?? "";
  for (const note of NOTES) expect(column).not.toContain(note);
  expect(turns()).toHaveLength(1);
  expect(
    log().querySelectorAll("details, [aria-expanded=false]:not([data-slot=turn-summary])"),
  ).toHaveLength(0);
  // a surface drawn on the way is an output of the turn: it stays in the column
  expect(within(log()).getByText("What the drawing holds")).toBeTruthy();
  // the panel has every sentence, among the steps, in the order they were said
  await waitFor(() => expect(rows().filter((r) => r.startsWith("note"))).toHaveLength(6));
  const seen = rows();
  expect(seen.filter((r) => r.startsWith("note"))).toEqual(NOTES.map((n) => `note: ${n}`));
  // time order: a note before the tool call it announced
  expect(seen[0]).toBe(`note: ${NOTES[0]}`);
  expect(seen.indexOf(`note: ${NOTES[3]}`)).toBeGreaterThan(
    seen.findIndex((r) => r.includes("run_checks")),
  );
}

describe("the answer in the chat and the working text in Activity", () => {
  it("coder-notes: the owner's coder chat is one answer in the column and six notes among the steps", async () => {
    const id = await makeThread("coder-notes draw something in node");
    shell(id);
    await expectTheOwnersChat();
    // the line of the turn counts the steps, not the notes
    const line = within(log()).getByRole("button", { name: /^Adam's steps: \d+ steps/ });
    expect(line.getAttribute("aria-label")).toMatch(/^Adam's steps: 11 steps/);
    expect(line.getAttribute("aria-label")).toContain("1 failed");
  });

  it("coder-notes-legacy: the same turn with no word marked reads the same, by the rule for old logs", async () => {
    const id = await makeThread("coder-notes-legacy draw something in node");
    shell(id);
    await expectTheOwnersChat();
  });

  it("Copy copies the answer, not the sentences said on the way", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const id = await makeThread("coder-notes draw something in node");
    shell(id);
    await waitFor(() => expect(answers()).toHaveLength(1));
    fireEvent.click(within(log()).getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledTimes(1);
    const copied = writeText.mock.calls[0]?.[0] as string;
    expect(copied).toContain(ANSWER);
    for (const note of NOTES) expect(copied).not.toContain(note);
  });

  it("a question that ends the turn is the answer: the words and the wait for the person stay in the column", async () => {
    const id = await makeThread("ask about branches");
    shell(id);
    await waitFor(() => expect(answers()).toHaveLength(1));
    expect(answers()[0]?.textContent).toContain("Which branch?");
    expect(within(log()).getByText("Waiting for your reply")).toBeTruthy();
  });

  it("a plain agent's one message is the answer, with no note and no line for steps it never took", async () => {
    const id = await makeThread("talk to me");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(answers()).toHaveLength(1));
    expect(answers()[0]?.textContent).toContain("Plan: add a test");
    expect(activity().querySelectorAll('li[data-kind="note"]')).toHaveLength(0);
  });

  it("coder-notes-running: unmarked words show as a draft of the answer until a step starts after them, then fold", async () => {
    const id = await makeThread("coder-notes-running fix the redirect", true);
    shell(id);
    // the run waits: the words are the last thing the agent said, so they show
    await waitFor(() => expect(answers()).toHaveLength(1));
    expect(answers()[0]?.textContent).toContain("Let me look at the failing test first.");
    expect(activity().querySelectorAll('li[data-kind="note"]')).toHaveLength(0);

    // the run goes on: a step starts after them and they fold into Activity, then the answer
    const released = await realFetch(`${base}/__mock/release?thread=${id}`, { method: "POST" });
    expect(released.status).toBe(204);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(answers()).toHaveLength(1));
    expect(answers()[0]?.textContent).toContain("Fixed: the redirect no longer loops.");
    // the turn is over: the ticker is gone and the words are nowhere in the column
    await waitFor(() =>
      expect(log().textContent).not.toContain("Let me look at the failing test first."),
    );
    expect(log().querySelector('[data-slot="turn-ticker"]')).toBeNull();
    await waitFor(() =>
      expect(rows()).toEqual([
        "note: Let me look at the failing test first.",
        expect.stringContaining("read_file"),
      ]),
    );
  });

  it("coder-notes-hold: while the turn runs the column has no answer yet, and the line shows the last working sentence", async () => {
    const id = await makeThread("coder-notes-hold draw something", true);
    shell(id);
    const ticker = await waitFor(() => {
      const t = log().querySelector<HTMLElement>('[data-slot="turn-ticker"]');
      expect(t?.textContent).toBe("All 7 tests pass. Now I'll export the drawing.");
      return t as HTMLElement;
    });
    // quiet: not announced, not a control
    expect(ticker.closest("[aria-live]")).toBeNull();
    expect(answers()).toHaveLength(0);
    // the earlier sentences are nowhere in the column, and the last one only in the ticker
    for (const note of NOTES.slice(0, 4)) expect(log().textContent).not.toContain(note);
    expect((log().textContent ?? "").split(NOTES[4] as string)).toHaveLength(2);
    expect(rows().filter((r) => r.startsWith("note"))).toHaveLength(5);
  });
});
