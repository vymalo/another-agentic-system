// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TurnSteps } from "@/features/chat/lib/step-tree";
import { type ExpansionState, NO_EXPANSION } from "./expansion";
import { type StepsFocus, StepsPane } from "./steps-pane";
import {
  actorPart,
  assistant,
  openCodeTurn,
  RUNNING_VIEW,
  statusPart,
  stepPart,
  textPart,
  turnsOf,
} from "./testing";

let scrolled: Element[] = [];

beforeEach(() => {
  scrolled = [];
  Element.prototype.scrollIntoView = vi.fn(function (this: Element) {
    scrolled.push(this);
  });
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

function Harness({
  turns,
  focus = null,
  live = false,
  initial = NO_EXPANSION,
  spy,
}: {
  turns: readonly TurnSteps[];
  focus?: StepsFocus | null;
  live?: boolean;
  initial?: ExpansionState;
  spy?: (next: ExpansionState) => void;
}) {
  const [expanded, setExpanded] = useState(initial);
  return (
    <StepsPane
      turns={turns}
      focus={focus}
      live={live}
      expanded={expanded}
      onExpandedChange={(next) => {
        spy?.(next);
        setExpanded(next);
      }}
    />
  );
}

const headings = () => screen.getAllByRole("heading", { level: 3 });
const toggle = (name: RegExp | string) => screen.getByRole("button", { name });
const opencode = () => toggle(/^OpenCode/);
describe("a turn's steps are collapsed by default, one line per level", () => {
  it("shows the depth-1 steps and a step with children as one line: its name, how many, a failure", () => {
    render(<Harness turns={turnsOf([openCodeTurn(14, { failAt: 2 })])} />);
    expect(screen.getByText("Started working")).toBeTruthy();
    const line = opencode();
    expect(line.getAttribute("aria-expanded")).toBe("false");
    expect(line.textContent).toBe("OpenCode· 14 steps");
    // collapsed: a failure inside is still said, with an icon and a word
    const chip = within(line.closest("li") as HTMLElement).getByText("1 failed");
    expect(chip.closest('[data-slot="failed-chip"]')).not.toBeNull();
    expect(screen.queryByText("step 13")).toBeNull();
  });

  it("opens the latest three on the first click, and the failed one with them", () => {
    render(<Harness turns={turnsOf([openCodeTurn(24, { failAt: 5 })])} />);
    fireEvent.click(opencode());
    expect(opencode().getAttribute("aria-expanded")).toBe("true");
    const list = screen.getByRole("list", { name: "Steps of OpenCode" });
    expect(
      within(list)
        .getAllByRole("listitem")
        .map((li) => li.querySelector("[title]")?.getAttribute("title")),
    ).toEqual(["Command failed", "step 21", "step 22", "step 23"]);
    // the failed one is in view, so the chip is not repeated on the open step's own line
    expect(opencode().parentElement?.querySelector('[data-slot="failed-chip"]')).toBeNull();
  });

  it("'Show 10 more' lists ten earlier ones, again and again, and then the rest", () => {
    render(<Harness turns={turnsOf([openCodeTurn(24, { failAt: 5 })])} />);
    fireEvent.click(opencode());
    const count = () => screen.getByRole("list", { name: "Steps of OpenCode" }).children.length;
    expect(count()).toBe(4);
    fireEvent.click(screen.getByRole("button", { name: /Show 10 more steps of OpenCode/ }));
    expect(count()).toBe(14); // the latest 13 and the failed one, which is among the older
    fireEvent.click(screen.getByRole("button", { name: /Show 10 more steps of OpenCode/ }));
    expect(count()).toBe(23); // all but the very first
    fireEvent.click(screen.getByRole("button", { name: /Show 1 more steps of OpenCode/ }));
    expect(count()).toBe(24);
    expect(screen.queryByRole("button", { name: /more steps of OpenCode/ })).toBeNull();
  });

  it("says how many are left when fewer than ten are", () => {
    render(<Harness turns={turnsOf([openCodeTurn(14)])} />);
    fireEvent.click(opencode());
    fireEvent.click(screen.getByRole("button", { name: /Show 10 more steps of OpenCode/ }));
    fireEvent.click(screen.getByRole("button", { name: /Show 1 more steps of OpenCode/ }));
    expect(screen.queryByRole("button", { name: /more steps/ })).toBeNull();
    expect(screen.getByRole("list", { name: "Steps of OpenCode" }).children).toHaveLength(14);
  });

  it("closes a step again with the same button, and keeps what the person opened when the pane remounts", () => {
    const seen: ExpansionState[] = [];
    const turns = turnsOf([openCodeTurn(6, { id: "kept" })]);
    const { unmount } = render(<Harness turns={turns} spy={(next) => seen.push(next)} />);
    fireEvent.click(opencode());
    expect(screen.getAllByText(/^step \d$/).length).toBeGreaterThan(0);
    const opened = seen.at(-1) as ExpansionState;
    unmount();
    render(<Harness turns={turns} initial={opened} />);
    expect(opencode().getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(opencode());
    expect(opencode().getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("step 5")).toBeNull();
  });

  it("repeats the failure chip at every collapsed level of a deep tree", () => {
    const parts = [
      statusPart("working"),
      stepPart("A", "completed", { kind: "subagent", label: "Planner" }, 1),
      stepPart("B", "completed", { kind: "subagent", label: "Builder" }, 2, ["A"]),
      stepPart("C", "failed", { kind: "command", label: "make all" }, 3, ["A", "B"]),
    ];
    render(<Harness turns={turnsOf([assistant(parts)])} />);
    const planner = toggle(/Planner/).closest("li") as HTMLElement;
    expect(within(planner).getAllByText("1 failed")).toHaveLength(1);
    fireEvent.click(toggle(/Planner/));
    const builder = toggle(/Builder/).closest("li") as HTMLElement;
    // Planner is open now (its failed descendant is two levels down, under a collapsed one)
    expect(within(builder).getAllByText("1 failed")).toHaveLength(1);
    fireEvent.click(toggle(/Builder/));
    expect(
      within(screen.getByRole("list", { name: "Steps of Builder" })).getByText("Command failed"),
    ).toBeTruthy();
  });

  it("draws a command in a monospace box, a failure with its detail, and the words for a screen reader", () => {
    render(<Harness turns={turnsOf([openCodeTurn(3, { failAt: 1 })])} />);
    fireEvent.click(opencode());
    expect(screen.getByText("npm test 1", { selector: "code" })).toBeTruthy();
    // the detail the agent gave, under the command (the turn's header has the chip with the same words)
    expect(document.querySelector('[data-slot="step-detail"]')?.textContent).toBe("1 failed");
    expect(screen.getByText(/^Failed:$/).className).toContain("sr-only");
  });
});

describe("a long level is a scroll box that draws only what is in view", () => {
  beforeEach(() => {
    // jsdom has no layout: the box is 360 px high
    Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
      configurable: true,
      value: 360,
    });
    Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, value: 320 });
  });
  afterEach(() => {
    Reflect.deleteProperty(HTMLElement.prototype, "offsetHeight");
    Reflect.deleteProperty(HTMLElement.prototype, "offsetWidth");
  });

  it("is a plain list up to 50 rows", () => {
    const turns = turnsOf([openCodeTurn(60)]);
    const key = `${turns[0]?.turnId}/T/oc`;
    render(<Harness turns={turns} initial={{ ...NO_EXPANSION, nodes: new Map([[key, 50]]) }} />);
    expect(document.querySelector('[data-slot="steps-scroll"]')).toBeNull();
    expect(screen.getByRole("list", { name: "Steps of OpenCode" }).children).toHaveLength(50);
  });

  it("is virtualised above 50: a scroll box with a window of the rows", () => {
    const turns = turnsOf([openCodeTurn(120)]);
    const key = `${turns[0]?.turnId}/T/oc`;
    render(<Harness turns={turns} initial={{ ...NO_EXPANSION, nodes: new Map([[key, 120]]) }} />);
    const box = document.querySelector<HTMLElement>('[data-slot="steps-scroll"]');
    expect(box).not.toBeNull();
    expect(box?.getAttribute("aria-label")).toBe("Steps of OpenCode, scrolls");
    expect(box?.getAttribute("tabindex")).toBe("0");
    const list = within(box as HTMLElement).getByRole("list", { name: "Steps of OpenCode" });
    const drawn = list.children.length;
    expect(drawn).toBeGreaterThan(0);
    expect(drawn).toBeLessThan(40);
    // every row knows its place among all of them
    expect(list.children[0]?.getAttribute("aria-setsize")).toBe("120");
    expect(list.children[0]?.getAttribute("aria-posinset")).toBe("1");
  });
});

describe("which turns the pane lists and opens", () => {
  const three = () =>
    turnsOf([
      openCodeTurn(2, { id: "t1" }),
      assistant([textPart("only words")], undefined, "t2"),
      openCodeTurn(3, { id: "t3", revision: "coder-r47" }),
    ]);

  it("lists the turns that did something, in order, numbered as the chat numbers them", () => {
    render(<Harness turns={three()} />);
    expect(headings().map((h) => h.textContent)).toEqual([
      "Turn 1 · Coder · 3s",
      "Turn 3 · Coder · 4s",
    ]);
  });

  it("opens the last turn when nothing is running, the others show their header only", () => {
    render(<Harness turns={three()} />);
    const [one, other] = headings().map((h) => within(h).getByRole("button"));
    expect(one?.getAttribute("aria-expanded")).toBe("false");
    expect(other?.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getAllByRole("button", { name: /OpenCode/ })).toHaveLength(1);
  });

  it("opens a turn by its header and closes one, and stops choosing for the person after that", () => {
    const turns = three();
    const { rerender } = render(<Harness turns={turns} />);
    fireEvent.click(within(headings()[0] as HTMLElement).getByRole("button"));
    expect(screen.getAllByRole("button", { name: /OpenCode/ })).toHaveLength(2);
    fireEvent.click(within(headings()[1] as HTMLElement).getByRole("button"));
    expect(screen.getAllByRole("button", { name: /OpenCode/ })).toHaveLength(1);
    expect(
      within(headings()[0] as HTMLElement)
        .getByRole("button")
        .getAttribute("aria-expanded"),
    ).toBe("true");
    rerender(<Harness turns={turns} />);
    expect(
      within(headings()[1] as HTMLElement)
        .getByRole("button")
        .getAttribute("aria-expanded"),
    ).toBe("false");
  });

  it("is an empty tab, with its words, when no turn has steps", () => {
    render(<Harness turns={turnsOf([assistant([textPart("hello")])])} />);
    expect(screen.getByText("What the agents do shows up here")).toBeTruthy();
    expect(screen.queryAllByRole("heading", { level: 3 })).toHaveLength(0);
  });

  it("lists the turn that runs even before it has a step", () => {
    const running = assistant([actorPart()], { type: "running" }, "t9");
    render(<Harness live turns={turnsOf([running], RUNNING_VIEW)} />);
    expect(headings().map((h) => h.textContent)).toEqual(["Turn 1 · Coder"]);
  });
});

describe("while the thread runs", () => {
  const turns = (view = RUNNING_VIEW) =>
    turnsOf([openCodeTurn(2, { id: "t1" }), openCodeTurn(1, { id: "t2", running: true })], view);

  it("opens the live turn and not the others, and keeps the step it is on in view", () => {
    render(<Harness live turns={turns()} />);
    const [one, two] = headings().map((h) => within(h).getByRole("button"));
    expect(one?.getAttribute("aria-expanded")).toBe("false");
    expect(two?.getAttribute("aria-expanded")).toBe("true");
    // the sub-agent spins: it is the step the pane keeps in view
    expect(scrolled.some((el) => el.getAttribute("data-state") === "live")).toBe(true);
  });

  it("follows a turn that starts while the person has chosen nothing", () => {
    const first = turnsOf([openCodeTurn(2, { id: "t1", running: true })], RUNNING_VIEW);
    const { rerender } = render(<Harness live turns={first} />);
    expect(
      within(headings()[0] as HTMLElement)
        .getByRole("button")
        .getAttribute("aria-expanded"),
    ).toBe("true");
    rerender(<Harness live turns={turns()} />);
    const [one, two] = headings().map((h) => within(h).getByRole("button"));
    expect(one?.getAttribute("aria-expanded")).toBe("false");
    expect(two?.getAttribute("aria-expanded")).toBe("true");
  });

  it("stops following when the person picks a turn: the live turn does not take it back", () => {
    const first = turnsOf([openCodeTurn(2, { id: "t1" })], RUNNING_VIEW);
    const { rerender } = render(<Harness live turns={first} />);
    // the person closes the turn they were reading
    fireEvent.click(within(headings()[0] as HTMLElement).getByRole("button"));
    rerender(<Harness live turns={turns()} />);
    const [one, two] = headings().map((h) => within(h).getByRole("button"));
    expect(one?.getAttribute("aria-expanded")).toBe("false");
    expect(two?.getAttribute("aria-expanded")).toBe("false");
  });

  it("stops keeping the live step in view once the person scrolls", () => {
    const { rerender } = render(<Harness live turns={turns()} />);
    scrolled = [];
    fireEvent.wheel(document.querySelector('[data-slot="steps-pane"]') as HTMLElement);
    rerender(
      <Harness
        live
        turns={turnsOf(
          [openCodeTurn(2, { id: "t1" }), openCodeTurn(2, { id: "t2", running: true })],
          RUNNING_VIEW,
        )}
      />,
    );
    expect(scrolled).toEqual([]);
  });
});

describe("a request to show a turn", () => {
  const turns = () => turnsOf([openCodeTurn(2, { id: "t1" }), openCodeTurn(2, { id: "t2" })]);

  it("opens that turn, scrolls to it, focuses its header, and marks it for a moment", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "requestAnimationFrame", "cancelAnimationFrame"] });
    try {
      const view = turns();
      render(<Harness turns={view} focus={{ turnId: "t1", key: 1 }} />);
      const header = headings()[0] as HTMLElement;
      expect(within(header).getByRole("button").getAttribute("aria-expanded")).toBe("true");
      expect(scrolled.some((el) => el.getAttribute("data-turn") === "t1")).toBe(true);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(50);
      });
      expect(document.activeElement).toBe(header);
      expect(header.className).toContain("bg-brand/10");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1600);
      });
      expect(header.className).not.toContain("bg-brand/10");
    } finally {
      vi.useRealTimers();
    }
  });

  it("focuses it again on a second request for the same turn (a new key)", async () => {
    const view = turns();
    const { rerender } = render(<Harness turns={view} focus={{ turnId: "t1", key: 1 }} />);
    const header = headings()[0] as HTMLElement;
    await waitFor(() => expect(document.activeElement).toBe(header));
    (document.activeElement as HTMLElement).blur();
    scrolled = [];
    rerender(<Harness turns={view} focus={{ turnId: "t1", key: 1 }} />);
    expect(scrolled).toEqual([]); // the same request is not acted on twice
    rerender(<Harness turns={view} focus={{ turnId: "t1", key: 2 }} />);
    await waitFor(() => expect(document.activeElement).toBe(header));
    expect(scrolled.some((el) => el.getAttribute("data-turn") === "t1")).toBe(true);
  });

  it("does not act on a request again when the pane is shown again (the shell keeps the key)", () => {
    const view = turns();
    const seen: ExpansionState[] = [];
    const { unmount } = render(
      <Harness turns={view} focus={{ turnId: "t1", key: 1 }} spy={(e) => seen.push(e)} />,
    );
    unmount();
    scrolled = [];
    render(
      <Harness
        turns={view}
        focus={{ turnId: "t1", key: 1 }}
        initial={seen.at(-1) as ExpansionState}
      />,
    );
    expect(scrolled).toEqual([]);
    expect(document.activeElement).toBe(document.body);
  });

  it("opens a turn that the person had closed, and leaves the others as they are", () => {
    const view = turns();
    const { rerender } = render(<Harness turns={view} />);
    // t2 (the last) is open by default, t1 is closed
    rerender(<Harness turns={view} focus={{ turnId: "t1", key: 1 }} />);
    const [one, two] = headings().map((h) => within(h).getByRole("button"));
    expect(one?.getAttribute("aria-expanded")).toBe("true");
    expect(two?.getAttribute("aria-expanded")).toBe("true");
  });

  it("has nothing to focus for a turn it does not list, and says so by doing nothing", () => {
    render(<Harness turns={turns()} focus={{ turnId: "gone", key: 1 }} />);
    expect(scrolled).toEqual([]);
  });
});

describe("what a reader of the page gets", () => {
  it("is a section per turn named by its header, with each level a list", () => {
    render(<Harness turns={turnsOf([openCodeTurn(3)])} />);
    const section = screen.getByRole("region", { name: /^Turn 1/ });
    expect(section.tagName).toBe("SECTION");
    expect(within(section).getByRole("list", { name: "Steps of turn 1" })).toBeTruthy();
    // not an ARIA tree: no roving tabindex, every toggle is a button in the tab order
    expect(document.querySelector('[role="tree"]')).toBeNull();
    for (const button of screen.getAllByRole("button")) {
      expect(button.getAttribute("type")).toBe("button");
      expect(button.getAttribute("tabindex")).toBeNull();
    }
    expect(document.querySelector("[aria-live]")).toBeNull();
  });

  it("says the state of a step in words before its name, for a step that is not simply done", () => {
    render(<Harness turns={turnsOf([openCodeTurn(2, { failAt: 0 })])} />);
    fireEvent.click(opencode());
    const failed = screen.getByText("Command failed").closest("li") as HTMLElement;
    expect(failed.textContent).toContain("Failed: ");
  });
});
