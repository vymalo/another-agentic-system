// @vitest-environment jsdom
import {
  act,
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { AttemptCounter } from "@/features/chat/components/attempt-counter";
import { StateBadge } from "@/features/chat/components/state-badge";
import { mountSurfaces, stubLayout } from "@/features/chat/components/surface/testing";
import { type GoldenFrame, loadGolden, THREAD_ID } from "@/features/chat/lib/agui/testing";
import type { CheckContent } from "@/features/chat/lib/agui/vymalo";
import { CheckCard } from "./check-card";
import { FINDING_PREVIEW, FINDINGS_SHOWN } from "./findings-list";
import { ReworkDivider } from "./rework-divider";

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
afterEach(cleanup);

const check = (over: Partial<CheckContent> = {}): CheckContent => ({
  source: "agent_checks",
  attempt: 1,
  status: "failed",
  stale: false,
  findings: [],
  ...over,
});

const card = () => screen.getByRole("region", { name: /^Check: / });

describe("the check card", () => {
  it("says passed, failed or pending in words, with the source, the attempt and the short commit", () => {
    const { rerender } = render(
      <CheckCard
        data={check({
          status: "failed",
          commit: "0000000000000000000000000000000000000001",
          summary: "1 test failed",
          findings: ["tests::login fails: expected 200, got 500"],
          actor: { type: "system", name: "orchestrator" },
        })}
      />,
    );
    expect(card().getAttribute("aria-label")).toBe("Check: Agent checks, attempt 1, failed");
    const view = within(card());
    expect(view.getByText("Failed")).toBeTruthy();
    expect(view.getByText("Agent checks")).toBeTruthy();
    expect(view.getByText("Attempt 1")).toBeTruthy();
    expect(view.getByText("0000000").getAttribute("title")).toBe(
      "0000000000000000000000000000000000000001",
    );
    expect(view.getByText("1 test failed")).toBeTruthy();
    expect(view.getByText("Findings (1)")).toBeTruthy();
    expect(view.getByText("tests::login fails: expected 200, got 500")).toBeTruthy();
    expect(view.getByText("orchestrator")).toBeTruthy();

    rerender(<CheckCard data={check({ status: "passed", source: "ci", name: "build" })} />);
    expect(card().getAttribute("aria-label")).toBe("Check: CI, attempt 1, passed");
    expect(within(card()).getByText("Passed")).toBeTruthy();
    expect(within(card()).getByText("CI")).toBeTruthy();
    expect(within(card()).getByText(/build/)).toBeTruthy();
    expect(within(card()).queryByText(/Findings/)).toBeNull();

    rerender(<CheckCard data={check({ status: "pending", source: "verifier" })} />);
    expect(card().getAttribute("aria-label")).toBe("Check: Verifier, attempt 1, pending");
    expect(within(card()).getByText("Pending")).toBeTruthy();
    expect(card().getAttribute("data-status")).toBe("pending");
  });

  it("a stale answer is muted and marked, and says it decided nothing", () => {
    render(<CheckCard data={check({ source: "ci", status: "failed", stale: true })} />);
    expect(card().getAttribute("data-stale")).toBe("true");
    expect(card().getAttribute("aria-label")).toBe("Check: CI, attempt 1, failed, stale");
    expect(within(card()).getByText("Stale")).toBeTruthy();
    expect(within(card()).getByText(/It decided nothing/)).toBeTruthy();
    expect(card().className).toContain("border-dashed");
    cleanup();
    render(<CheckCard data={check()} />);
    expect(card().getAttribute("data-stale")).toBeNull();
    expect(within(card()).queryByText("Stale")).toBeNull();
  });

  it("findings are text: markup and markdown are shown as the characters they are", () => {
    const hostile = [
      "<script>window.pwned = 1</script>",
      "**bold** and _italic_ and `code`",
      '<img src="x" onerror="window.pwned = 2">',
      "[click](javascript:window.pwned=3) https://example.com/auto-link",
      "# heading\n- item\n\n> quote",
      "<a href='https://evil.example'>go</a>",
    ];
    const { container } = render(
      <CheckCard
        data={check({
          summary: "<b>summary</b> **not bold**",
          commit: "<script>x</script>",
          findings: hostile,
        })}
      />,
    );
    // every finding is on the page as the characters it is (the list folds after five)
    fireEvent.click(within(card()).getByRole("button", { name: `Show all 6 findings` }));
    const items = within(card()).getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual(hostile);
    expect(within(card()).getByText("<b>summary</b> **not bold**")).toBeTruthy();
    // and none of it became an element
    expect(
      container.querySelector("script, img, a, b, strong, em, h1, blockquote, iframe"),
    ).toBeNull();
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
    // the only `code` element is the commit, and it holds text, cut short
    const codes = container.querySelectorAll("code");
    expect(codes).toHaveLength(1);
    expect(codes[0]?.textContent).toBe("<script>x</s…");
  });

  it("a long finding is cut, with a control that shows all of it and takes it back", () => {
    const long = `${"x".repeat(FINDING_PREVIEW)}TAIL-OF-THE-FINDING`;
    render(<CheckCard data={check({ findings: [long, "short one"] })} />);
    const item = within(card()).getAllByRole("listitem")[0] as HTMLElement;
    expect(item.textContent).not.toContain("TAIL-OF-THE-FINDING");
    expect(item.textContent).toContain("…");
    const more = within(item).getByRole("button", { name: "Show more" });
    expect(more.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(more);
    expect(item.textContent).toContain("TAIL-OF-THE-FINDING");
    expect(
      within(item).getByRole("button", { name: "Show less" }).getAttribute("aria-expanded"),
    ).toBe("true");
    fireEvent.click(within(item).getByRole("button", { name: "Show less" }));
    expect(item.textContent).not.toContain("TAIL-OF-THE-FINDING");
    // a short finding has no control
    const second = within(card()).getAllByRole("listitem")[1] as HTMLElement;
    expect(within(second).queryByRole("button")).toBeNull();
  });

  it("a long list is folded after five findings, with a control for the rest", () => {
    const many = Array.from({ length: FINDINGS_SHOWN + 3 }, (_, i) => `finding number ${i}`);
    render(<CheckCard data={check({ findings: many })} />);
    expect(within(card()).getAllByRole("listitem")).toHaveLength(FINDINGS_SHOWN);
    expect(within(card()).queryByText("finding number 7")).toBeNull();
    fireEvent.click(within(card()).getByRole("button", { name: "Show all 8 findings" }));
    expect(within(card()).getAllByRole("listitem")).toHaveLength(FINDINGS_SHOWN + 3);
    expect(within(card()).getByText("finding number 7")).toBeTruthy();
    fireEvent.click(within(card()).getByRole("button", { name: "Show fewer findings" }));
    expect(within(card()).getAllByRole("listitem")).toHaveLength(FINDINGS_SHOWN);
  });

  it("two identical findings are both listed", () => {
    render(<CheckCard data={check({ findings: ["same", "same"] })} />);
    expect(within(card()).getAllByText("same")).toHaveLength(2);
  });
});

describe("the rework divider", () => {
  const rework = (findings: number[]) => ({
    attempt: 2,
    maxAttempts: 3,
    findings: findings.map((n, i) => ({
      source: i === 0 ? "agent_checks" : "ci",
      findings: Array.from({ length: n }, (_, k) => `f${k}`),
    })),
  });

  it("names the attempt that starts and how many findings the agent was sent back with", () => {
    const { container } = render(<ReworkDivider data={rework([1])} />);
    expect(screen.getByText("Attempt 2 of 3: sent back with 1 finding")).toBeTruthy();
    expect(
      container.querySelector("[data-slot='rework-divider']")?.getAttribute("data-attempt"),
    ).toBe("2");
    cleanup();
    render(<ReworkDivider data={rework([2, 3])} />);
    expect(screen.getByText("Attempt 2 of 3: sent back with 5 findings")).toBeTruthy();
    cleanup();
    render(<ReworkDivider data={{ attempt: 3, maxAttempts: 3, findings: [] }} />);
    expect(screen.getByText("Attempt 3 of 3: sent back with 0 findings")).toBeTruthy();
  });
});

describe("the attempt counter and the verifying badge", () => {
  it("shows nothing without a job (no gate)", () => {
    const { container } = render(<AttemptCounter job={null} />);
    expect(container.textContent).toBe("");
  });

  it("shows 2/3, and says 'Attempt 2 of 3' to a screen reader", () => {
    const { container } = render(
      <AttemptCounter job={{ attempt: 2, maxAttempts: 3, gate: ["agent_checks"] }} />,
    );
    expect(screen.getByText("Attempt 2/3")).toBeTruthy();
    expect(screen.getByText("Attempt 2 of 3").className).toContain("sr-only");
    expect(container.querySelector("[aria-hidden='true']")?.textContent).toBe("Attempt 2/3");
  });

  it("the badge says Verifying, and what that means to a screen reader; it is not the Working colour", () => {
    const { rerender } = render(<StateBadge state="verifying" />);
    const badge = screen.getByRole("status", { name: /^Thread state:/ });
    expect(badge.textContent).toBe("Verifying");
    expect(badge.getAttribute("aria-label")).toBe("Thread state: Verifying the agent's work");
    expect(badge.className).toContain("text-verifying");
    rerender(<StateBadge state="working" />);
    const working = screen.getByRole("status", { name: /^Thread state:/ });
    expect(working.getAttribute("aria-label")).toBe("Thread state: Working");
    expect(working.className).not.toContain("text-verifying");
  });
});

// ---- through the runtime: the renderers as the transcript draws them ---------------------------

type Mounted = ReturnType<typeof mountSurfaces>;

async function feed(m: Mounted, frames: GoldenFrame[]) {
  await act(async () => {
    m.stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
}

const actor = { "vymalo.actor": { type: "system", name: "orchestrator" } };

/** A run of the given activities (each `[messageId, activityType, content]`), closed by success. */
function activities(list: [string, string, unknown][], first = 1): GoldenFrame[] {
  let seq = first;
  return [
    { event: { type: "RUN_STARTED", threadId: THREAD_ID, runId: "run-1", protocolVersion: "1.0" } },
    ...list.map(([messageId, activityType, content]) => ({
      id: seq++,
      event: {
        type: "ACTIVITY_SNAPSHOT",
        messageId,
        activityType,
        content,
        replace: true,
        metadata: actor,
      },
    })),
    {
      id: seq,
      event: {
        type: "RUN_FINISHED",
        threadId: THREAD_ID,
        runId: "run-1",
        outcome: { type: "success" },
      },
    },
  ];
}

const regions = () => screen.queryAllByRole("region", { name: /^Check: / });
const dividers = () => document.querySelectorAll("[data-slot='rework-divider']");

describe("the renderers in the transcript", () => {
  it("verify-green: the failed check, the divider and the passed check, in that order", async () => {
    const m = mountSurfaces();
    await feed(m, loadGolden("verify-green"));
    await waitFor(() => expect(regions()).toHaveLength(2));
    const [first, second] = regions() as [HTMLElement, HTMLElement];
    expect(first.getAttribute("aria-label")).toBe("Check: Agent checks, attempt 1, failed");
    expect(second.getAttribute("aria-label")).toBe("Check: Agent checks, attempt 2, passed");
    expect(within(first).getByText("tests::login fails: expected 200, got 500")).toBeTruthy();
    expect(within(second).queryByText(/Findings/)).toBeNull();
    const divider = dividers();
    expect(divider).toHaveLength(1);
    expect(divider[0]?.textContent).toBe("Attempt 2 of 3: sent back with 1 finding");
    // document order: the failed card, the divider, the passed card
    expect(
      first.compareDocumentPosition(divider[0] as Element) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      (divider[0] as Element).compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    // and the agent's second attempt is in the same transcript after the divider
    expect(screen.getAllByText("Working").length).toBe(2);
    m.agent.stop();
  });

  it("verify-red: three failed cards and two dividers, the last attempt without one", async () => {
    const m = mountSurfaces();
    await feed(m, loadGolden("verify-red"));
    await waitFor(() => expect(regions()).toHaveLength(3));
    expect(regions().map((r) => r.getAttribute("aria-label"))).toEqual([
      "Check: Agent checks, attempt 1, failed",
      "Check: Agent checks, attempt 2, failed",
      "Check: Agent checks, attempt 3, failed",
    ]);
    expect([...dividers()].map((d) => d.textContent)).toEqual([
      "Attempt 2 of 3: sent back with 1 finding",
      "Attempt 3 of 3: sent back with 1 finding",
    ]);
    m.agent.stop();
  });

  it("a card is replaced in place by its id: pending, then the answer (one card, not two)", async () => {
    const m = mountSurfaces();
    const id = "check-1-1-ci";
    await feed(m, [
      ...activities([
        [id, "vymalo.check", { source: "ci", attempt: 1, status: "pending", name: "build" }],
      ]).slice(0, 2),
    ]);
    await waitFor(() => expect(regions()).toHaveLength(1));
    expect(regions()[0]?.getAttribute("data-status")).toBe("pending");
    await feed(m, [
      {
        id: 2,
        event: {
          type: "ACTIVITY_SNAPSHOT",
          messageId: id,
          activityType: "vymalo.check",
          replace: true,
          content: {
            source: "ci",
            attempt: 1,
            status: "failed",
            name: "build",
            findings: ["build broke"],
          },
          metadata: actor,
        },
      },
    ]);
    await waitFor(() => expect(regions()[0]?.getAttribute("data-status")).toBe("failed"));
    expect(regions()).toHaveLength(1);
    expect(within(regions()[0] as HTMLElement).getByText("build broke")).toBeTruthy();
    m.agent.stop();
  });

  it("a stale check has an id of its own and is shown next to the current one, muted", async () => {
    const m = mountSurfaces();
    await feed(
      m,
      activities([
        ["check-2-2-ci", "vymalo.check", { source: "ci", attempt: 2, status: "passed" }],
        ["evt-9", "vymalo.check", { source: "ci", attempt: 1, status: "failed", stale: true }],
      ]),
    );
    await waitFor(() => expect(regions()).toHaveLength(2));
    expect(regions().map((r) => r.getAttribute("data-stale"))).toEqual([null, "true"]);
    m.agent.stop();
  });

  it("a malformed payload draws nothing and does not crash; the rest of the run still renders", async () => {
    const m = mountSurfaces();
    await feed(
      m,
      activities([
        ["evt-1", "vymalo.check", {}],
        ["evt-2", "vymalo.check", { source: "ci", attempt: 1 }],
        ["evt-3", "vymalo.check", { source: 7, attempt: "one", status: "who knows" }],
        [
          "evt-4",
          "vymalo.check",
          { source: "ci", attempt: 1, status: "failed", findings: { not: "a list" } },
        ],
        ["evt-5", "vymalo.rework", { attempt: "two" }],
        ["evt-6", "vymalo.rework", {}],
        [
          "evt-7",
          "vymalo.check",
          { source: "ci", attempt: 1, status: "passed", futureField: { x: [1, 2] } },
        ],
        ["evt-8", "vymalo.future", { anything: true }],
      ]),
    );
    // the run finished; of everything only the two check payloads that hold a source, an attempt
    // and a status are drawn, and the unknown fields are ignored
    await waitFor(() => expect(regions()).toHaveLength(2));
    expect(regions().map((r) => r.getAttribute("data-status"))).toEqual(["failed", "passed"]);
    expect(within(regions()[0] as HTMLElement).queryByText(/Findings/)).toBeNull();
    expect(dividers()).toHaveLength(0);
    m.agent.stop();
  });

  it("findings that arrive as markup are text all the way from the wire", async () => {
    const m = mountSurfaces();
    await feed(
      m,
      activities([
        [
          "check-1-1-agent_checks",
          "vymalo.check",
          {
            source: "agent_checks",
            attempt: 1,
            status: "failed",
            summary: "<img src=x onerror=alert(1)>",
            findings: ["<script>window.pwned = 1</script>", "**bold**", "[x](javascript:alert(1))"],
          },
        ],
        [
          "rework-2",
          "vymalo.rework",
          {
            attempt: 2,
            maxAttempts: 3,
            findings: [{ source: "agent_checks<script>", findings: ["<script>x</script>"] }],
          },
        ],
      ]),
    );
    await waitFor(() => expect(regions()).toHaveLength(1));
    const log = screen.getByRole("log", { name: "Conversation" });
    expect(within(log).getByText("<script>window.pwned = 1</script>")).toBeTruthy();
    expect(within(log).getByText("**bold**")).toBeTruthy();
    expect(within(log).getByText("[x](javascript:alert(1))")).toBeTruthy();
    expect(log.querySelector("script, img, strong, iframe, a[href^='javascript']")).toBeNull();
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
    m.agent.stop();
  });

  it("a hold while verifying is an interrupt the user can answer (no delivery failure, no subagent)", async () => {
    const m = mountSurfaces();
    const thread = { state: "blocked", target: { agentId: "plain" }, title: "t" };
    await feed(m, [
      {
        event: { type: "RUN_STARTED", threadId: THREAD_ID, runId: "run-1", protocolVersion: "1.0" },
      },
      {
        id: 1,
        event: {
          type: "ACTIVITY_SNAPSHOT",
          messageId: "check-1-1-ci",
          activityType: "vymalo.check",
          replace: true,
          content: { source: "ci", attempt: 1, status: "pending" },
          metadata: actor,
        },
      },
      {
        event: {
          type: "STATE_SNAPSHOT",
          snapshot: { thread, job: { attempt: 1, maxAttempts: 3, gate: ["ci"] } },
        },
      },
      {
        id: 2,
        event: {
          type: "RUN_FINISHED",
          threadId: THREAD_ID,
          runId: "run-1",
          outcome: {
            type: "interrupt",
            interrupts: [{ id: "int-2", reason: "input_required", message: "CI did not answer" }],
          },
        },
      },
    ]);
    await waitFor(() =>
      expect(m.runtime().unstable_getPendingInterrupts()).toMatchObject([
        { id: "int-2", reason: "input_required", message: "CI did not answer" },
      ]),
    );
    expect(m.agent.getSnapshot()).toMatchObject({ state: "blocked", job: { attempt: 1 } });
    expect(regions()).toHaveLength(1);
    m.agent.stop();
  });
});
