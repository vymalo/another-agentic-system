// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import { ThreadViewProvider } from "@/features/chat/components/thread-view";
import type { ApiThread } from "@/lib/api/types";
import { DEFAULT_OFFER } from "../../../../mock/sessions";
import type { SessionCard, SessionStartCard } from "../lib/cards";
import { CHILD_SESSIONS_URI } from "../lib/offer";
import { type MockPage, mockPage } from "../test-mock";

/*
 * The Subagents tab's content and the controller behind it (child-sessions/v1): who is listed and in
 * what order, the words of each row, "New subagent" and the reasons it is not available, the two
 * dialogs it opens (against the mock orchestrator) and the focus they give back, and when the tab is
 * there at all.
 */

let page: MockPage;
let SessionsProvider: typeof import("./sessions-provider").SessionsProvider;
let useSessions: typeof import("./sessions-provider").useSessions;
let SessionsPanel: typeof import("./sessions-panel").SessionsPanel;
let StartLine: typeof import("./start-line").StartLine;

beforeAll(async () => {
  page = await mockPage();
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  ({ SessionsProvider, useSessions } = await import("./sessions-provider"));
  ({ SessionsPanel } = await import("./sessions-panel"));
  ({ StartLine } = await import("./start-line"));
});
afterAll(() => page.restore());
afterEach(cleanup);

const PARENT = "00000000-0000-4000-8000-0000000000f0";
const id = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;

const row = (n: number, state: string, over: Record<string, unknown> = {}): ApiThread =>
  ({
    id: id(n),
    title: `Task ${n}`,
    state: "done",
    target: { agentId: "adam" },
    nestedUnder: PARENT,
    childOf: { threadId: PARENT, session: `s${n}`, agent: "planner", state, keepOpen: n % 2 === 1 },
    ...over,
  }) as unknown as ApiThread;

const caps = (entry: unknown = DEFAULT_OFFER): AgentCapabilities => ({
  status: "ready",
  supports: () => true,
  entry: (uri) => (uri === CHILD_SESSIONS_URI ? entry : undefined),
  refresh: vi.fn(),
});

type Setup = {
  rows?: ApiThread[];
  cards?: Map<string, SessionCard>;
  starts?: Map<string, SessionStartCard>;
  capabilities?: AgentCapabilities;
  thread?: ApiThread | null;
  readOnly?: boolean;
  parent?: string;
  refreshRows?: () => void;
  extra?: ReactNode;
};

function Probe() {
  const s = useSessions();
  return <p data-testid="visible">{String(s?.visible)}</p>;
}

function tree(s: Setup) {
  return (
    <ThreadViewProvider
      value={{
        state: "done",
        waiting: false,
        agentId: "adam",
        sessions: s.cards ?? new Map(),
        sessionStarts: s.starts ?? new Map(),
      }}
    >
      <SessionsProvider
        threadId={s.parent ?? PARENT}
        thread={s.thread ?? null}
        rows={s.rows ?? []}
        capabilities={s.capabilities ?? caps()}
        tier={undefined}
        readOnly={s.readOnly ?? false}
        refreshRows={s.refreshRows ?? (() => {})}
      >
        <Probe />
        <SessionsPanel />
        {s.extra}
      </SessionsProvider>
    </ThreadViewProvider>
  );
}
const mount = (s: Setup = {}) => render(tree(s));
const visible = () => screen.getByTestId("visible").textContent;

describe("when the tab is there", () => {
  it("is there when the agent offers subagents, even with none yet, and says so", () => {
    mount();
    expect(visible()).toBe("true");
    expect(screen.getByText(/No subagents yet/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "New subagent" })).toBeTruthy();
  });

  it("is there for a thread that has subagents when the card no longer offers them, with no New button", () => {
    mount({ rows: [row(1, "idle")], capabilities: caps(null) });
    expect(visible()).toBe("true");
    expect(screen.getByText("Task 1")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "New subagent" })).toBeNull();
  });

  it("is not there for an agent that offers none and a thread with none", () => {
    mount({ capabilities: caps(null) });
    expect(visible()).toBe("false");
  });

  it("is not there when the card lists the extension badly (a card that lists it badly does not list it)", () => {
    mount({ capabilities: caps({ agents: [], maxOpen: 4 }) });
    expect(visible()).toBe("false");
  });

  it("is not there on a subagent's own thread", () => {
    mount({ thread: row(1, "idle") });
    expect(visible()).toBe("false");
  });

  it("is not there on the new-chat page", () => {
    const { container } = render(
      <SessionsProvider
        threadId={null}
        thread={null}
        rows={[]}
        capabilities={caps()}
        tier={undefined}
        readOnly={false}
        refreshRows={() => {}}
      >
        <Probe />
      </SessionsProvider>,
    );
    // no controller: nothing there to say anything about
    expect(container.textContent).toBe("undefined");
  });

  it("lists a read-only thread's subagents without Close or New", () => {
    mount({ rows: [row(1, "idle")], readOnly: true });
    expect(screen.getByText("Task 1")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Close/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "New subagent" })).toBeNull();
  });
});

describe("the rows", () => {
  it("lists the open ones first, each with its name, its description as text, its state in words and its mark", () => {
    mount({
      rows: [row(1, "closed"), row(2, "working"), row(3, "idle", { title: "<b>Plan</b> it" })],
      cards: new Map([
        [
          "session-9",
          {
            session: "s2",
            agent: "planner",
            description: "",
            keepOpen: false,
            state: "working",
            closing: false,
            closeAsked: false,
            round: 2,
          },
        ],
      ]),
    });
    const list = screen.getByRole("list", { name: "Subagents" });
    const rows = within(list).getAllByRole("listitem");
    expect(
      rows.map((r) => r.querySelector('[data-slot="subagent-description"]')?.textContent),
    ).toEqual(["Task 2", "<b>Plan</b> it"]);
    expect(rows[0]?.querySelector('[data-slot="subagent-state"]')?.textContent).toBe("Working");
    expect(rows[1]?.querySelector('[data-slot="subagent-state"]')?.textContent).toBe("Idle");
    expect(rows[1]?.querySelector("b")).toBeNull();
    expect(rows[0]?.textContent).toContain("Closes when done"); // Task 2: n even, keepOpen false
    expect(rows[1]?.textContent).toContain("Stays open");
  });

  it("folds the closed ones into 'n closed'", () => {
    mount({ rows: [row(1, "closed"), row(2, "closed"), row(3, "idle")] });
    const fold = screen.getByText("2 closed");
    expect(fold.closest("details")?.open).toBe(false);
    fireEvent.click(fold);
    expect(
      within(screen.getByRole("list", { name: "Closed subagents" })).getAllByRole("listitem"),
    ).toHaveLength(2);
    // a closed one has no Close
    expect(
      within(screen.getByRole("list", { name: "Closed subagents" })).queryByRole("button"),
    ).toBeNull();
  });

  it("links Open to the child's thread, made of a UUID only", () => {
    mount({
      rows: [row(1, "idle")],
      cards: new Map([
        [
          "session-9",
          {
            session: "s7",
            threadId: "../x",
            agent: "researcher",
            description: "d",
            keepOpen: false,
            state: "idle",
            closing: false,
            closeAsked: false,
          },
        ],
      ]),
    });
    const links = screen.getAllByRole("link", { name: /^Open/ });
    expect(links.map((l) => l.getAttribute("href"))).toEqual([`/threads/${id(1)}`]);
  });

  it("says 'Closing…' with a spinner for a close that waits, and still offers Close: it can be made stronger", () => {
    mount({
      rows: [
        row(1, "working", {
          childOf: {
            threadId: PARENT,
            session: "s1",
            agent: "planner",
            state: "working",
            keepOpen: true,
            closing: true,
          },
        }),
      ],
    });
    const li = screen.getByRole("listitem");
    expect(li.querySelector('[data-slot="subagent-state"]')?.textContent).toBe("Closing…");
    expect(li.querySelector("svg.animate-spin, svg[class*='animate-spin']")).toBeTruthy();
    expect(within(li).getByRole("button", { name: /^Close/ })).toBeTruthy();
  });

  it("offers no Close to a close that is on its way and has nothing to wait for", () => {
    mount({
      rows: [
        row(1, "idle", {
          childOf: {
            threadId: PARENT,
            session: "s1",
            agent: "planner",
            state: "idle",
            keepOpen: true,
            closeAsked: true,
          },
        }),
      ],
    });
    const li = screen.getByRole("listitem");
    expect(li.querySelector('[data-slot="subagent-state"]')?.textContent).toBe("Closing…");
    expect(within(li).queryByRole("button", { name: /^Close/ })).toBeNull();
  });

  it("says a subagent whose thread waits for the person is Waiting for you", () => {
    mount({ rows: [row(1, "working", { state: "blocked" })] });
    expect(screen.getByRole("listitem").textContent).toContain("Waiting for you");
  });

  it("lists a start that has no outcome yet as Starting…", () => {
    mount({
      starts: new Map([
        [
          "r1",
          {
            request: "r1",
            agent: "planner",
            description: "Plan the docs",
            keepOpen: false,
            state: "starting",
          },
        ],
      ]),
    });
    const li = screen.getByRole("listitem");
    expect(li.textContent).toContain("Plan the docs");
    expect(li.textContent).toContain("Starting…");
  });
});

describe("New subagent", () => {
  it("is disabled, with the reason in view, when maxOpen are open", () => {
    mount({
      rows: [1, 2, 3, 4].map((n) => row(n, "idle")),
    });
    const button = screen.getByRole("button", { name: "New subagent" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    const reason = screen.getByText("4 subagents are open. Close one to start another.");
    expect(button.getAttribute("aria-describedby")).toBe(reason.id);
  });

  it("is disabled, with its reason, while four starts wait", () => {
    const starts = new Map(
      [1, 2, 3, 4].map((n) => [
        `r${n}`,
        {
          request: `r${n}`,
          agent: "planner",
          description: `d${n}`,
          keepOpen: false,
          state: "starting" as const,
        },
      ]),
    );
    mount({ starts });
    expect(
      (screen.getByRole("button", { name: "New subagent" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(screen.getByText("4 subagents are starting. Wait for one to open.")).toBeTruthy();
  });

  it("is enabled below the limits, and closed subagents do not count", () => {
    mount({
      rows: [
        row(1, "closed"),
        row(2, "closed"),
        row(3, "closed"),
        row(4, "closed"),
        row(5, "idle"),
      ],
    });
    expect(
      (screen.getByRole("button", { name: "New subagent" }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });

  it("opens the dialog, reads the card again, and gives the focus back to the button when it closes", async () => {
    const capabilities = caps();
    mount({ capabilities });
    const button = screen.getByRole("button", { name: "New subagent" });
    button.focus();
    fireEvent.click(button);
    const dialog = await screen.findByRole("dialog", { name: "New subagent" });
    expect(capabilities.refresh).toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(button));
  });

  it("is withdrawn when a start is refused with no_child_sessions, and is back once the card says it offers them again", async () => {
    const parent = await page.api.answered();
    const capabilities = caps();
    const view = mount({ capabilities, parent });
    fireEvent.click(screen.getByRole("button", { name: "New subagent" }));
    await page.api.config("sessions=off");
    fireEvent.change(await screen.findByLabelText("What it is for"), { target: { value: "Late" } });
    fireEvent.change(screen.getByLabelText("What to ask it"), { target: { value: "Go" } });
    fireEvent.click(screen.getByRole("button", { name: "Start subagent" }));
    await waitFor(() =>
      expect(
        screen.getAllByText("This agent does not offer subagents right now.").length,
      ).toBeGreaterThan(0),
    );
    await page.api.config("sessions=on");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "New subagent" })).toBeNull());
    expect(capabilities.refresh).toHaveBeenCalled();
    // the card, read again, still lists it: a new reading brings the button back
    view.rerender(tree({ capabilities: caps({ ...DEFAULT_OFFER }), parent }));
    expect(await screen.findByRole("button", { name: "New subagent" })).toBeTruthy();
  });
});

describe("starting from the tab, then trying again", () => {
  it("closes the dialog on 202, keeps the draft until the outcome, and 'Try again' reopens it with the draft", async () => {
    const parent = await page.api.answered();
    await page.api.config("sessionStarts=refuse");
    const view = mount({ parent });
    fireEvent.click(screen.getByRole("button", { name: "New subagent" }));
    fireEvent.change(await screen.findByLabelText("What it is for"), {
      target: { value: "Plan the docs" },
    });
    fireEvent.change(screen.getByLabelText("What to ask it"), {
      target: { value: "Plan them all" },
    });
    fireEvent.click(screen.getByRole("checkbox", { name: /Close when done/ }));
    fireEvent.click(screen.getByRole("button", { name: "Start subagent" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // asked, and counted as waiting until the stream says its outcome
    const [event] = await page.api.of(parent, "child_session_start_asked");
    const request = String(event?.data.request);
    await page.api.until(
      async () => (await page.api.of(parent, "child_session_start_ended")).length === 1,
      "the outcome",
    );

    // the stream says it failed: the start line offers Try again, which reopens the dialog with the draft
    const failed: SessionStartCard = {
      request,
      agent: "planner",
      description: "Plan the docs",
      keepOpen: true,
      state: "failed",
      reason: "max_working: 4",
    };
    const line = <StartLine data={{ ...failed, card: `session-start-${request}`, home: true }} />;
    view.rerender(tree({ parent, starts: new Map([[request, failed]]), extra: line }));
    const again = await screen.findByRole("button", { name: "Try again" });
    again.focus();
    fireEvent.click(again);
    const dialog = await screen.findByRole("dialog", { name: "New subagent" });
    expect((within(dialog).getByLabelText("What it is for") as HTMLInputElement).value).toBe(
      "Plan the docs",
    );
    expect((within(dialog).getByLabelText("What to ask it") as HTMLTextAreaElement).value).toBe(
      "Plan them all",
    );
    expect(
      (within(dialog).getByRole("checkbox", { name: /Close when done/ }) as HTMLInputElement)
        .checked,
    ).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(again));
    await page.api.config("sessionStarts=open");
  });

  it("drops the draft once the start opened: the line has no Try again then", async () => {
    const parent = await page.api.answered();
    const view = mount({ parent });
    fireEvent.click(screen.getByRole("button", { name: "New subagent" }));
    fireEvent.change(await screen.findByLabelText("What it is for"), {
      target: { value: "Plan it" },
    });
    fireEvent.change(screen.getByLabelText("What to ask it"), { target: { value: "Go" } });
    fireEvent.click(screen.getByRole("button", { name: "Start subagent" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const [event] = await page.api.of(parent, "child_session_start_asked");
    const request = String(event?.data.request);
    const opened: SessionStartCard = {
      request,
      agent: "planner",
      description: "Plan it",
      keepOpen: false,
      state: "opened",
      threadId: id(77),
      session: "s1",
    };
    const failedLater: SessionStartCard = { ...opened, state: "failed", reason: "x" };
    view.rerender(tree({ parent, starts: new Map([[request, opened]]) }));
    // a later `failed` for a request that opened cannot happen; if the draft is gone there is nothing to retry
    view.rerender(
      tree({
        parent,
        starts: new Map([[request, failedLater]]),
        extra: (
          <StartLine data={{ ...failedLater, card: `session-start-${request}`, home: true }} />
        ),
      }),
    );
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });
});

describe("closing from the tab", () => {
  it("asks first, applies the 202's state at once, and refreshes the list", async () => {
    const parent = await page.api.answered();
    const { child } = await page.api.opened(parent, { keepOpen: true, text: "hold on" });
    await page.api.until(
      async () => (await page.api.thread(child)).childOf?.state === "working",
      "working",
    );
    const thread = await page.api.thread(child);
    const refreshRows = vi.fn();
    mount({ parent, rows: [thread], refreshRows });
    const close = screen.getByRole("button", { name: "Close planner" });
    close.focus();
    fireEvent.click(close);
    const dialog = await screen.findByRole("dialog", { name: "Close subagent" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Close when this round ends" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // the rail has not said it yet: the row says it already
    const li = screen.getByRole("listitem");
    expect(li.querySelector('[data-slot="subagent-state"]')?.textContent).toBe("Closing…");
    expect(refreshRows).toHaveBeenCalled();
    await page.api.release(child);
  });
});
