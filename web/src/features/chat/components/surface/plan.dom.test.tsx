// @vitest-environment jsdom
import { act, cleanup, configure, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { OWN_CATALOG } from "@/features/chat/lib/a2ui/catalog";
import { ANNOUNCE_AFTER_MS, ANNOUNCE_SETTLE_MS, forgetPlans } from "./plan-announcer";
import { actionRun, mountSurfaces, resetSeq, stubLayout, surfaceRun } from "./testing";

/*
 * A plan (catalog version 8, docs/api/ui-catalog-v1.md, "A surface sent again" and "The plan"), through the
 * real runtime and transcript: one surface that the agent sends again each time the plan changes is ONE
 * checklist in the conversation, where it first stood, drawn from the newest copy, and said politely to
 * assistive technology when it changes. A surface that is not a plan keeps the rule it always had.
 */

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(() => {
  resetSeq();
  forgetPlans();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

type Rec = Record<string, unknown>;
type Mounted = ReturnType<typeof mountSurfaces>;

async function feed(m: Mounted, frames: ReturnType<typeof surfaceRun>) {
  await act(async () => {
    m.stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
}

const item = (label: string, state: string, note?: string) => ({
  label,
  state,
  ...(note ? { note } : {}),
});
/** The operations of one copy of the plan: the whole surface, under the same id every time. */
const copy = (items: Rec[], surfaceId = "plan-run-7"): Rec[] => [
  { version: "1.0", createSurface: { surfaceId, catalogId: OWN_CATALOG.catalogId } },
  {
    version: "1.0",
    updateComponents: {
      surfaceId,
      components: [
        { id: "root", component: "Column", children: ["steps"] },
        { id: "steps", component: "Checklist", title: "Fix the parser", items },
      ],
    },
  },
];
const mount = (host = {}) =>
  mountSurfaces(host, {}, undefined, { catalogVersion: OWN_CATALOG.version });
const lists = () => document.querySelectorAll("[data-slot=checklist]");
const status = () => document.querySelector("[data-slot=plan-status]");

describe("a plan is one checklist that updates where it stands", () => {
  it("is one checklist after five updates in a run, showing the last", async () => {
    const m = mount();
    const steps = [
      [item("Read the test", "doing"), item("Fix it", "todo")],
      [item("Read the test", "done"), item("Fix it", "doing")],
      [item("Read the test", "done"), item("Fix it", "doing", "The second branch")],
      [item("Read the test", "done"), item("Fix it", "done")],
      [item("Read the test", "done"), item("Fix it", "done"), item("Open the PR", "doing")],
    ];
    await feed(
      m,
      surfaceRun(
        steps.map((s) => copy(s)),
        { end: "success" },
      ),
    );
    expect(lists()).toHaveLength(1);
    const plan = screen.getByRole("region", { name: "Plan from plain" });
    expect(within(plan).getAllByRole("listitem")).toHaveLength(3);
    expect(plan.textContent).toContain("2 of 3 done");
    m.agent.stop();
  });

  it("is one checklist across runs: the first copy stands and draws the newest, a later copy draws nothing", async () => {
    const m = mount();
    await feed(m, surfaceRun([copy([item("A", "doing"), item("B", "todo")])], { end: "success" }));
    await feed(
      m,
      surfaceRun([copy([item("A", "done"), item("B", "doing")])], {
        runId: "run-2",
        user: false,
        end: "success",
      }),
    );
    await feed(
      m,
      surfaceRun([copy([item("A", "done"), item("B", "done"), item("C", "todo")])], {
        runId: "run-3",
        user: false,
        end: "success",
      }),
    );
    await waitFor(() => expect(screen.getByText("2 of 3 done")).toBeTruthy());
    expect(lists()).toHaveLength(1);
    expect(screen.queryByText("This interface was updated further down.")).toBeNull();
    expect(screen.getAllByRole("region", { name: "Plan from plain" })).toHaveLength(1);
    m.agent.stop();
  });

  it("says a dropped item in words, leaves it out of the count, and shows its note", async () => {
    const m = mount();
    await feed(
      m,
      surfaceRun(
        [
          copy([
            item("Read the test", "done"),
            item("Port it", "dropped", "Out of scope: the CLI does not parse"),
            item("Open the PR", "todo"),
          ]),
        ],
        { end: "success" },
      ),
    );
    const plan = screen.getByRole("region", { name: "Plan from plain" });
    expect(plan.textContent).toContain("1 of 2 done");
    const dropped = within(plan)
      .getAllByRole("listitem")
      .find((li) => li.getAttribute("data-state") === "dropped");
    expect(dropped?.textContent).toContain("Dropped");
    expect(dropped?.textContent).toContain("Out of scope: the CLI does not parse");
    m.agent.stop();
  });

  it("a plan that holds a form is refused whole, rule action", async () => {
    const m = mount();
    const bad = [
      {
        version: "1.0",
        createSurface: { surfaceId: "plan-x", catalogId: OWN_CATALOG.catalogId },
      },
      {
        version: "1.0",
        updateComponents: {
          surfaceId: "plan-x",
          components: [
            { id: "root", component: "Column", children: ["ask"] },
            {
              id: "ask",
              component: "Choices",
              questions: [{ id: "q", question: "Q?", type: "text" }],
            },
          ],
        },
      },
    ];
    await feed(m, surfaceRun([bad], { end: "success" }));
    expect(document.querySelector("[data-rule]")?.getAttribute("data-rule")).toBe("action");
    expect(screen.getByText(/is a plan \(its id starts with plan-\)/)).toBeTruthy();
    m.agent.stop();
  });

  it("a surface that is not a plan keeps the rule it had: the earlier copy says it was updated", async () => {
    const m = mount();
    await feed(m, surfaceRun([copy([item("A", "todo")], "s1")], { end: "success" }));
    await feed(
      m,
      surfaceRun([copy([item("A", "done")], "s1")], {
        runId: "run-2",
        user: false,
        end: "success",
      }),
    );
    await waitFor(() =>
      expect(screen.getByText("This interface was updated further down.")).toBeTruthy(),
    );
    expect(lists()).toHaveLength(1);
    m.agent.stop();
  });
});

describe("the plan is announced politely, once, and only when it changes", () => {
  it("says nothing for the first copy, then 'Plan updated: 2 of 3 done' after it has been quiet", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const m = mount();
    await feed(
      m,
      surfaceRun([copy([item("A", "done"), item("B", "doing"), item("C", "todo")])], {
        end: "success",
      }),
    );
    const region = status();
    expect(region?.getAttribute("role")).toBe("status");
    expect(region?.getAttribute("aria-live")).toBe("polite");
    await act(async () => {
      vi.advanceTimersByTime(ANNOUNCE_AFTER_MS * 2);
    });
    expect(region?.textContent).toBe("");
    await feed(
      m,
      surfaceRun([copy([item("A", "done"), item("B", "done"), item("C", "doing")])], {
        runId: "run-2",
        user: false,
        end: "success",
      }),
    );
    // not before it has been quiet
    expect(status()?.textContent).toBe("");
    await act(async () => {
      vi.advanceTimersByTime(ANNOUNCE_AFTER_MS + 50);
    });
    expect(status()?.textContent).toBe("Plan updated: 2 of 3 done");
    m.agent.stop();
  });

  it("a burst of updates is one announcement, of the last state", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const m = mount();
    await feed(m, surfaceRun([copy([item("A", "doing"), item("B", "todo")])], { end: "success" }));
    await act(async () => {
      vi.advanceTimersByTime(ANNOUNCE_SETTLE_MS);
    });
    const burst = [
      [item("A", "done"), item("B", "todo")],
      [item("A", "done"), item("B", "doing")],
      [item("A", "done"), item("B", "done")],
    ];
    for (const [i, items] of burst.entries()) {
      await feed(
        m,
        surfaceRun([copy(items)], { runId: `run-${i + 2}`, user: false, end: "success" }),
      );
    }
    await act(async () => {
      vi.advanceTimersByTime(ANNOUNCE_AFTER_MS + 50);
    });
    const said = status()?.querySelectorAll("p");
    expect(said).toHaveLength(1);
    expect(said?.[0]?.textContent).toBe("Plan updated: 2 of 2 done");
    m.agent.stop();
  });

  it("is not touched by what the agent says: a reply that streams leaves the region as it was", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const m = mount({ canSend: true });
    await feed(m, surfaceRun([copy([item("A", "doing")])], { end: "success" }));
    await act(async () => {
      vi.advanceTimersByTime(ANNOUNCE_SETTLE_MS);
    });
    await feed(
      m,
      surfaceRun([copy([item("A", "done")])], { runId: "run-2", user: false, end: "success" }),
    );
    await act(async () => {
      vi.advanceTimersByTime(ANNOUNCE_AFTER_MS + 50);
    });
    const before = status()?.querySelector("p");
    expect(before?.textContent).toBe("Plan updated: 1 of 1 done");
    await feed(
      m,
      actionRun({ surfaceId: "plan-run-7", name: "answer", context: {} }, { runId: "run-3" }),
    );
    await act(async () => {
      vi.advanceTimersByTime(ANNOUNCE_AFTER_MS * 3);
    });
    // the very same node: nothing was said again
    expect(status()?.querySelector("p")).toBe(before);
    m.agent.stop();
  });
});

describe("a plan never rewrites what the person saw: one drawing per stretch of the thread", () => {
  const regions = () => screen.queryAllByRole("region", { name: "Plan from plain" });
  const FRAMES = (a: Rec[], b: Rec[]) => {
    resetSeq();
    return [
      ...surfaceRun([a], { runId: "run-1", messageId: "a2ui-3", end: "success" }),
      // the person writes (user: true) and the plan is sent again, in a new run, under a new activity key
      ...surfaceRun([b], { runId: "run-2", messageId: "a2ui-9", end: "success" }),
    ];
  };

  it("a plan sent again in a new job is a new drawing; the earlier one stays as it was, under its line", async () => {
    const m = mount();
    await feed(
      m,
      FRAMES(
        copy([item("A", "doing"), item("B", "todo")]),
        copy([item("A", "done"), item("B", "doing")]),
      ),
    );
    await waitFor(() => expect(regions()).toHaveLength(2));
    const [first, second] = regions();
    expect(first?.textContent).toContain("0 of 2 done");
    expect(first?.textContent).toContain("This plan was updated further down.");
    expect(second?.textContent).toContain("1 of 2 done");
    expect(second?.textContent).not.toContain("updated further down");
    // only the live drawing is announced: one status region
    expect(document.querySelectorAll("[data-slot=plan-status]")).toHaveLength(1);
    expect(second?.querySelector("[data-slot=plan-status]")).not.toBeNull();
    m.agent.stop();
  });

  it("ORIGINAL words the person answered stay above their message after a reload; REWRITTEN is a drawing of its own", async () => {
    const original = copy([item("ORIGINAL: delete the cache", "todo")]);
    const rewritten = copy([item("REWRITTEN: delete the database", "todo")]);
    const first = mount();
    await feed(first, FRAMES(original, rewritten));
    await waitFor(() => expect(regions()).toHaveLength(2));
    first.agent.stop();
    cleanup();
    // a reload: the page is drawn again from the same transcript
    const again = mount();
    await feed(again, FRAMES(original, rewritten));
    await waitFor(() => expect(regions()).toHaveLength(2));
    const [one, two] = regions();
    expect(one?.textContent).toContain("ORIGINAL: delete the cache");
    expect(one?.textContent).not.toContain("REWRITTEN");
    expect(two?.textContent).toContain("REWRITTEN: delete the database");
    expect(
      one && two && Boolean(one.compareDocumentPosition(two) & Node.DOCUMENT_POSITION_FOLLOWING),
    ).toBe(true);
    again.agent.stop();
  });

  it("a hundred copies with no message of a person between them are one drawing, and at most one announcement", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const m = mount();
    const copies = Array.from({ length: 100 }, (_, k) =>
      copy([item("A", k < 50 ? "doing" : "done", `copy ${k}`), item("B", "todo")]),
    );
    await feed(m, surfaceRun(copies, { end: "success" }));
    await act(async () => {
      vi.advanceTimersByTime(ANNOUNCE_AFTER_MS * 2);
    });
    expect(regions()).toHaveLength(1);
    expect(lists()).toHaveLength(1);
    expect(status()?.querySelectorAll("p").length ?? 0).toBeLessThanOrEqual(1);
    m.agent.stop();
  });

  it("a viewer who may not act (read-only, a share link) sees the plan, and nothing in it can send", async () => {
    const m = mount({ readOnly: "You can read this thread; it is not yours.", canSend: false });
    await feed(m, FRAMES(copy([item("A", "doing")]), copy([item("A", "done")])));
    await waitFor(() => expect(regions()).toHaveLength(2));
    for (const region of regions()) {
      expect(within(region).queryAllByRole("button")).toHaveLength(0);
      expect(within(region).queryAllByRole("textbox")).toHaveLength(0);
    }
    expect(regions()[1]?.textContent).toContain("1 of 1 done");
    m.agent.stop();
  });
});
