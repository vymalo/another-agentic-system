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
let REVOKE_AFTER_MS: number;

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
  ({ REVOKE_AFTER_MS } = await import("@/features/chat/lib/export-thread"));
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
/** The right-hand panel's Activity tab: the steps of every agent turn, as a tree. */
const activity = () =>
  within(screen.getByRole("complementary", { name: "Thread details" })).getByRole("tabpanel", {
    name: "Activity",
  });
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
/** `coder · coder-r47` at the top of each agent turn, in order. */
const actorLabels = () =>
  [...log().querySelectorAll('[data-slot="actor-label"]')].map((e) => e.textContent);
/** "Export JSON" is an item of the thread's overflow menu: open the menu, find the item. */
async function exportItem(): Promise<HTMLElement> {
  const trigger = await screen.findByRole("button", { name: "Thread options" });
  if (trigger.getAttribute("aria-expanded") !== "true") {
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
  }
  return screen.findByRole("menuitem", { name: /Export JSON|Exporting…/ });
}

/** The agent picker of the top bar: "Agent: Coder", a menu button. */
const agentPicker = () => screen.findByRole("button", { name: /^Agent:/ });
/** Opens the agent menu from the keyboard, as `exportItem` opens the thread's. */
async function openAgentMenu(): Promise<HTMLElement> {
  const trigger = await agentPicker();
  if (trigger.getAttribute("aria-expanded") !== "true") {
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
  }
  return screen.findByRole("menu");
}
const radio = (menu: HTMLElement, name: RegExp) =>
  within(menu).getByRole("menuitemradio", { name });

describe("ChatShell over AG-UI", () => {
  it("the new-thread page offers the agents in the header's menu and, for the coder, its releases", async () => {
    shell(null);
    expect((await agentPicker()).textContent).toContain("Coder");
    const menu = await openAgentMenu();
    // the agents, the first checked, each with what it does
    expect(radio(menu, /^Coder/).getAttribute("aria-checked")).toBe("true");
    expect(radio(menu, /^Reviewer/).getAttribute("aria-checked")).toBe("false");
    expect(radio(menu, /^Coder/).textContent).toContain("Implements a change");
    // the coder's releases, in the same menu
    await waitFor(() =>
      expect(radio(menu, /^production/).getAttribute("aria-checked")).toBe("true"),
    );
    expect(radio(menu, /^production/).textContent).toContain("coder-r47");
    // choosing the reviewer closes the menu and the releases are gone
    fireEvent.click(radio(menu, /^Reviewer/));
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect((await agentPicker()).textContent).toContain("Reviewer");
    expect(within(await openAgentMenu()).queryByRole("group", { name: "Release" })).toBeNull();
  });

  it("opening the menu reads the agents again (releases are the card right now)", async () => {
    shell(null);
    await agentPicker();
    const reads = () => calls.filter((c) => c === "GET /api/agents 200").length;
    const before = reads();
    expect(before).toBeGreaterThan(0);
    await openAgentMenu();
    await waitFor(() => expect(reads()).toBeGreaterThan(before));
  });

  it("a thread's header names its agent and offers the others as a new chat, not as a switch", async () => {
    const id = await makeThread("Implement the thing");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const trigger = await agentPicker();
    expect(trigger.textContent).toContain("Coder");
    // the title is the page's heading, next to the picker
    await waitFor(() =>
      expect(screen.getByRole("heading", { level: 1 }).textContent).toContain(
        "Implement the thing",
      ),
    );
    const menu = await openAgentMenu();
    expect(radio(menu, /^Coder/).getAttribute("aria-checked")).toBe("true");
    // the other agents start a new chat with them; nothing here changes the thread's agent
    expect(within(menu).queryByRole("menuitemradio", { name: /^Reviewer/ })).toBeNull();
    const link = within(
      within(menu).getByRole("group", { name: "Start a new chat with" }),
    ).getByRole("menuitem", { name: /^Reviewer/ });
    expect(link.getAttribute("href")).toBe("/?agent=reviewer");
  });

  it("a thread has the panel's toggle in its header, and the new chat has none", async () => {
    shell(null);
    await agentPicker();
    expect(screen.queryByRole("button", { name: "Thread details" })).toBeNull();
    cleanup();

    // a window of 1024 px: the panel is a sheet there, closed until asked for
    Object.defineProperty(window, "innerWidth", {
      value: 1024,
      configurable: true,
      writable: true,
    });
    const id = await makeThread("Implement the thing");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const toggle = screen.getByRole("button", { name: "Thread details" });
    expect(toggle.getAttribute("aria-controls")).toBe("thread-panel");
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    const dialog = await screen.findByRole("dialog", { name: "Thread details" });
    // the Sources tab holds the pull request the thread's agent opened
    fireEvent.mouseDown(within(dialog).getByRole("tab", { name: /^Sources/ }));
    within(dialog)
      .getByRole("tab", { name: /^Sources/ })
      .focus();
    const link = await within(dialog).findByRole("link", { name: /^acme\/demo#1/ });
    expect(link.getAttribute("href")).toBe("https://github.com/acme/demo/pull/1");
  });

  it("a new chat opened from a link starts with the agent it names", async () => {
    window.history.replaceState(null, "", "/?agent=reviewer");
    try {
      shell(null);
      await waitFor(async () => expect((await agentPicker()).textContent).toContain("Reviewer"));
    } finally {
      window.history.replaceState(null, "", "/");
    }
  });

  it("a first message mints a UUIDv7, posts it, and goes to the thread", async () => {
    shell(null);
    await agentPicker();
    const menu = await openAgentMenu();
    await waitFor(() =>
      expect(radio(menu, /^production/).getAttribute("aria-checked")).toBe("true"),
    );
    fireEvent.click(radio(menu, /^staging/));
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "echo hello" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(push).toHaveBeenCalledTimes(1));
    expect(push.mock.calls[0]?.[0]).toMatch(
      /^\/threads\/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    expect(calls.filter((c) => c.startsWith("POST /agui/agents"))).toEqual([
      "POST /agui/agents/coder 200",
    ]);
    // nothing of the legacy interaction API
    expect(calls.some((c) => c.startsWith("POST /api/threads"))).toBe(false);
  });

  it("a rejected first message shows the problem and gives the text back once", async () => {
    shell(null);
    await agentPicker();
    failing = {
      key: "POST /agui/agents/coder",
      status: 400,
      detail: "text must be 1 to 100000 characters",
    };
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "keep me" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByText("text must be 1 to 100000 characters");
    await waitFor(() =>
      expect((screen.getByLabelText("Message") as HTMLTextAreaElement).value).toBe("keep me"),
    );
    await new Promise((r) => setTimeout(r, 100)); // and it stays once
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).value).toBe("keep me");
    expect(push).not.toHaveBeenCalled();
  });

  it("a finished thread is replayed from the connect stream: transcript, PR, Done, and the composer stays open", async () => {
    const id = await makeThread("Implement the thing");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const transcript = within(log());
    await waitFor(() => transcript.getByText("Implement the thing"));
    // the steps are the panel's, the chat has the one line of the turn, then the pull request as a card
    await waitFor(() => within(activity()).getByText("Opened pull request #1"));
    expect(within(activity()).getByText("Started working")).toBeTruthy();
    expect(transcript.queryByText("Started working")).toBeNull();
    expect(transcript.queryByRole("list", { name: "Steps" })).toBeNull();
    expect(
      transcript.getByRole("button", { name: /^Coder's steps: 2 steps.* Show in the side panel$/ }),
    ).toBeTruthy();
    const pr = transcript.getByRole("link", { name: /pull request acme\/demo#1/i });
    expect(pr.getAttribute("href")).toBe("https://github.com/acme/demo/pull/1");
    expect(transcript.getByText("echo: Implement the thing")).toBeTruthy();
    // the agent's name and revision, once, at the top of its turn
    expect(actorLabels()).toEqual(["coder · coder-r47"]);
    // a thread never locks (ADR 0020): the box is there, ready for the next request
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    expect(box.disabled).toBe(false);
    expect(box.placeholder).toBe("Send a follow-up…");
    expect(screen.queryByText(/This thread is done/)).toBeNull();
    expect(screen.queryByText(/Start a new thread/)).toBeNull();
    // replay: one connect, and the finished thread needs no more than that
    await new Promise((r) => setTimeout(r, 50));
    expect(calls.filter((c) => c.includes("/connect"))).toEqual([
      `GET /agui/threads/${id}/connect 200`,
    ]);
  });

  it("a turn with nested steps is one line in the chat, and the line opens the panel on that turn", async () => {
    const id = await makeThread("steps run the tests");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const transcript = within(log());
    const line = await transcript.findByRole("button", {
      name: "Coder's steps: 3 steps, 1 failed. Show in the side panel",
    });
    // one line: no list of steps and none of the step's words in the conversation
    expect(transcript.queryByRole("list", { name: "Steps" })).toBeNull();
    expect(transcript.queryByText("npm test")).toBeNull();
    expect(transcript.queryByText("OpenCode")).toBeNull();
    expect(transcript.getAllByRole("button", { name: /steps:/ })).toHaveLength(1);
    // the failure is said in the line, with its words
    expect(within(line).getByText("1 failed")).toBeTruthy();
    expect(line.getAttribute("aria-expanded")).toBe("false");

    // the panel lists the tree: the sub-agent as one line, its command inside
    const turn = screen.getByRole("region", { name: /^Turn 1/ });
    const opencode = within(turn).getByRole("button", { name: /^OpenCode/ });
    expect(within(turn).queryByText("Command failed")).toBeNull();
    fireEvent.click(opencode);
    expect(within(turn).getByText("Command failed")).toBeTruthy();

    fireEvent.click(line);
    await waitFor(() => expect(line.getAttribute("aria-expanded")).toBe("true"));
    await waitFor(() =>
      expect(document.activeElement).toBe(within(turn).getByRole("heading", { level: 3 })),
    );
  });

  it("each turn's line focuses its own turn in the panel", async () => {
    const id = await makeThread("steps run the tests");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => within(log()).getByText("steps run the tests"));
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "steps once more" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(within(log()).getAllByRole("button", { name: /steps:/ })).toHaveLength(2),
    );
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const [first, second] = within(log()).getAllByRole("button", { name: /steps:/ });
    const headings = () => within(activity()).getAllByRole("heading", { level: 3 });
    expect(headings().map((h) => h.textContent?.slice(0, 6))).toEqual(["Turn 1", "Turn 2"]);
    fireEvent.click(first as HTMLElement);
    await waitFor(() => expect(document.activeElement).toBe(headings()[0]));
    fireEvent.click(second as HTMLElement);
    await waitFor(() => expect(document.activeElement).toBe(headings()[1]));
    // a second click on the same line asks again: the focus comes back to the header
    (document.activeElement as HTMLElement).blur();
    fireEvent.click(second as HTMLElement);
    await waitFor(() => expect(document.activeElement).toBe(headings()[1]));
  });

  it("a follow-up after Done starts the next job in the same conversation: both jobs stay in the transcript", async () => {
    const id = await makeThread("echo hi");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    // the badge can say Done from the thread's own resource while the connect stream is still
    // replaying the first job; a follow-up sent into that gap would land before its history
    await waitFor(() => within(log()).getByText("echo hi"));
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    expect(box.disabled).toBe(false);
    fireEvent.change(box, { target: { value: "echo and now the tests" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const transcript = within(log());
    await waitFor(() => transcript.getByText("echo and now the tests"));
    await waitFor(() =>
      expect(transcript.getAllByRole("link", { name: /pull request acme\/demo#1/i })).toHaveLength(
        2,
      ),
    );
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    // each job is a turn of its own
    expect(actorLabels()).toEqual(["coder · coder-r47", "coder · coder-r47"]);
    // the first job is still there, the second job's answer is under the second message
    expect(transcript.getAllByText("echo hi")).toHaveLength(1);
    expect(transcript.getAllByRole("link", { name: /pull request acme\/demo#1/i })).toHaveLength(2);
    // one POST, the follow-up: the first job was not sent again
    expect(calls.filter((c) => c.startsWith("POST /agui/agents"))).toEqual([
      "POST /agui/agents/coder 200",
    ]);
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).disabled).toBe(false);
  });

  it("a thread that was stopped takes the next message too", async () => {
    const id = await makeThread("slow work", "reviewer", true);
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Working…"));
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Stopped"));
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "echo go on" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => within(log()).getByText("echo go on"));
  });

  it("while the agent works the box is open for drafting, Enter does not send, and the button says Stop", async () => {
    const id = await makeThread("slow work", "reviewer", true);
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Working…"));
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    expect(box.disabled).toBe(false);
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    fireEvent.change(box, { target: { value: "a draft for later" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await new Promise((r) => setTimeout(r, 50));
    expect(calls.filter((c) => c.startsWith("POST /agui/agents"))).toEqual([]);
    expect(box.value).toContain("a draft for later");
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Stopped"));
    // the draft is still there, and the button is Send again
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).value).toBe(
      "a draft for later",
    );
    expect(screen.getByRole("button", { name: "Send" })).toBeTruthy();
  });

  it("Export JSON downloads the whole thread as a file, through the API client", async () => {
    const id = await makeThread("Implement the thing");
    const blobs: Blob[] = [];
    const revoked: string[] = [];
    const downloads: string[] = [];
    const createObjectURL = vi.fn((b: Blob) => {
      blobs.push(b);
      return "blob:export";
    });
    URL.createObjectURL = createObjectURL;
    URL.revokeObjectURL = (u: string) => void revoked.push(u);
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      downloads.push(`${this.download} ${this.href}`);
    });
    // The object URL must outlive a slow download: it is revoked REVOKE_AFTER_MS later, not at once.
    const revokers: Array<() => void> = [];
    const realSetTimeout = globalThis.setTimeout;
    const setTimeoutSpy = vi.spyOn(globalThis, "setTimeout").mockImplementation(((
      fn: TimerHandler,
      ms?: number,
      ...args: unknown[]
    ) => {
      if (ms === REVOKE_AFTER_MS && typeof fn === "function") {
        revokers.push(fn as () => void);
        return 0;
      }
      return realSetTimeout(fn, ms, ...args);
    }) as typeof setTimeout);
    try {
      shell(id);
      await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
      fireEvent.click(await exportItem());
      await waitFor(() => expect(downloads).toEqual([`thread-${id}.json blob:export`]));
      expect(calls).toContain(`GET /api/threads/${id}/export 200`);
      const [file] = blobs;
      if (!file) throw new Error("nothing was downloaded");
      const doc = JSON.parse(await file.text());
      expect(doc.format).toBe("another-agentic-system/thread-export");
      expect(doc.version).toBe(1);
      expect(doc.thread.id).toBe(id);
      expect(doc.events.length).toBe(doc.thread.lastSeq);
      expect(doc.events[0].kind).toBe("user_message");
      expect(REVOKE_AFTER_MS).toBeGreaterThanOrEqual(30_000);
      expect(revokers).toHaveLength(1);
      expect(revoked).toEqual([]);
      revokers[0]?.();
      expect(revoked).toEqual(["blob:export"]);
      // the menu item is ready for another one, and nothing went wrong
      expect((await exportItem()).getAttribute("aria-disabled")).toBeNull();
      expect(screen.queryByText(/Could not export/)).toBeNull();
    } finally {
      click.mockRestore();
      setTimeoutSpy.mockRestore();
    }
  });

  it("a refused export says why and downloads nothing", async () => {
    const id = await makeThread("Implement the thing");
    const createObjectURL = vi.fn(() => "blob:never");
    URL.createObjectURL = createObjectURL;
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    failing = {
      key: `GET /api/threads/${id}/export`,
      status: 503,
      detail: "storage is unavailable",
    };
    fireEvent.click(await exportItem());
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Could not export the thread: storage is unavailable");
    expect(createObjectURL).not.toHaveBeenCalled();
    // another try works once the server does
    failing = undefined;
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    try {
      fireEvent.click(await exportItem());
      await waitFor(() => expect(createObjectURL).toHaveBeenCalledTimes(1));
      await waitFor(() => expect(screen.queryByText(/Could not export/)).toBeNull());
    } finally {
      click.mockRestore();
    }
  });

  it("a failed export is not shown under the next thread the header is reused for", async () => {
    const first = await makeThread("Implement the thing");
    const second = await makeThread("Implement the other thing");
    const { rerender } = shell(first);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    failing = {
      key: `GET /api/threads/${first}/export`,
      status: 503,
      detail: "storage is unavailable",
    };
    fireEvent.click(await exportItem());
    await screen.findByRole("alert");
    // The same component instance now shows another thread: the failure is about the first.
    rerender(
      <TooltipProvider>
        <ChatShell threadId={second} />
      </TooltipProvider>,
    );
    await waitFor(() => expect(screen.queryByText(/Could not export/)).toBeNull());
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(screen.queryByRole("alert")).toBeNull();
    // and it does not come back when the person returns to the first
    rerender(
      <TooltipProvider>
        <ChatShell threadId={first} />
      </TooltipProvider>,
    );
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(screen.queryByText(/Could not export/)).toBeNull();
  });

  it("a blocked thread offers the question, and the answer is a resume, not a message", async () => {
    const id = await makeThread("ask pick a branch");
    shell(id);
    await screen.findByText("Waiting for your answer.");
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).placeholder).toBe("Reply…");
    // the question is the runtime's interrupt, which arrives a moment after the thread state does
    await screen.findByText(/Which branch\?/);
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "main" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const transcript = within(log());
    await waitFor(() => expect(transcript.getAllByText("answered: main")).toHaveLength(1));
    expect(transcript.getAllByText("main")).toHaveLength(1);
    // the mock answers 422 to a message together with a resume: one POST, and it was accepted
    expect(calls.filter((c) => c.startsWith("POST /agui/agents"))).toEqual([
      "POST /agui/agents/coder 200",
    ]);
  });

  it("an answer whose run the connect stream finishes before the POST answers still shows its reply", async () => {
    // The orchestrator's order: the run happens (and the connect stream shows it, to Done) while
    // the POST's own RUN_STARTED is still on its way. The finished thread pauses the connect
    // stream; that pause must not take the send, whose events are already here, with it.
    const id = await makeThread("ask pick a branch");
    shell(id);
    await screen.findByText(/Which branch\?/);
    let release = () => {};
    holding = {
      key: "POST /agui/agents/coder",
      until: new Promise<void>((r) => {
        release = r;
      }),
    };
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "main" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await new Promise((r) => setTimeout(r, 50)); // the paused stream has let go
    release();
    const transcript = within(log());
    await waitFor(() => expect(transcript.getAllByText("answered: main")).toHaveLength(1));
    expect(transcript.getAllByText("main")).toHaveLength(1);
  });

  it("a refused send shows the problem, keeps the text and leaves no message behind", async () => {
    const id = await makeThread("ask pick a branch");
    shell(id);
    await screen.findByText("Waiting for your answer.");
    await screen.findByText(/Which branch\?/);
    failing = {
      key: "POST /agui/agents/coder",
      status: 409,
      detail: "the thread is finished (Done); start a new thread",
    };
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "late" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByText("the thread is finished (Done); start a new thread");
    await waitFor(() =>
      expect((screen.getByLabelText("Message") as HTMLTextAreaElement).value).toBe("late"),
    );
    expect(within(log()).queryByText("late")).toBeNull();
  });

  it("an A2UI surface is drawn; its button sends the action (no message, no resume) and the scenario goes on", async () => {
    const id = await makeThread("ui pick one", "reviewer");
    shell(id);
    const ui = await screen.findByRole("region", { name: "Interface from reviewer" });
    await waitFor(() => expect(stateBadge().textContent).toBe("Your turn"));
    expect(within(ui).getByText("Pick one")).toBeTruthy();
    const goButton = await within(ui).findByRole("button", { name: "Go" });
    await waitFor(() => expect((goButton as HTMLButtonElement).disabled).toBe(false));
    // opening the thread sent nothing but the connect
    expect(calls.filter((c) => c.startsWith("POST"))).toEqual([]);
    fireEvent.click(goButton);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const transcript = within(log());
    await waitFor(() => expect(transcript.getAllByText("answered: ui-action go")).toHaveLength(1));
    // the person's click is a step of the turn, so the panel's
    expect(within(activity()).getByText("Chose")).toBeTruthy();
    // one POST, the action; the surface is still there, and its button is off now
    expect(calls.filter((c) => c.startsWith("POST /agui/agents"))).toEqual([
      "POST /agui/agents/reviewer 200",
    ]);
    expect(transcript.getAllByText("ui pick one")).toHaveLength(1);
    expect(
      (
        within(transcript.getByRole("region", { name: /^Interface from/ })).getByRole("button", {
          name: "Go",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    expect(screen.getByText(/This request is finished/)).toBeTruthy();
  });

  it("a finished thread shows its surface read-only, and the button says why", async () => {
    const id = await makeThread("ui pick one", "reviewer");
    // answer it from outside, as another tab would
    const res = await realFetch(`${base}/agui/agents/reviewer`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
      body: JSON.stringify({
        threadId: id,
        runId: "run-2",
        messages: [],
        forwardedProps: {
          a2uiAction: {
            userAction: { name: "go", surfaceId: "s1", sourceComponentId: "go", context: {} },
          },
        },
      }),
    });
    expect(res.status).toBe(200);
    await res.text();
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const ui = await screen.findByRole("region", { name: "Interface from reviewer" });
    expect((within(ui).getByRole("button", { name: "Go" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect(within(ui).getByText(/This request is finished/)).toBeTruthy();
    expect(calls.filter((c) => c.startsWith("POST"))).toEqual([]);
  });

  it("a refused action shows the problem, keeps the transcript, and the button works again", async () => {
    const id = await makeThread("ui pick one", "reviewer");
    shell(id);
    const goButton = await screen.findByRole("button", { name: "Go" });
    await waitFor(() => expect((goButton as HTMLButtonElement).disabled).toBe(false));
    failing = {
      key: "POST /agui/agents/reviewer",
      status: 409,
      detail: "a run is already open on this thread; wait for it to finish",
    };
    fireEvent.click(goButton);
    await screen.findByText("a run is already open on this thread; wait for it to finish");
    // the message before the action is still there: an action carries none, so none was taken back
    expect(within(log()).getByText("ui pick one")).toBeTruthy();
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Go" }) as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
  });

  it("Cancel asks the orchestrator; the run ends cancelled and the runtime shows it", async () => {
    const id = await makeThread("slow work", "reviewer", true);
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Working…"));
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Stopped"));
    expect(calls).toContain(`POST /api/threads/${id}/cancel 202`);
    await waitFor(() =>
      expect(within(activity()).getAllByText("Stopped").length).toBeGreaterThan(0),
    );
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    // stopped is not closed: the next message goes on
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).placeholder).toBe(
      "Tell the agent how to go on…",
    );
  });

  it("an unknown thread is not found, and its connect is not retried", async () => {
    shell("00000000-0000-7000-8000-00000000dead");
    await screen.findByText(/Thread not found/);
    await new Promise((r) => setTimeout(r, 50));
    expect(calls.filter((c) => c.includes("/connect"))).toEqual([
      "GET /agui/threads/00000000-0000-7000-8000-00000000dead/connect 404",
    ]);
  });
});
