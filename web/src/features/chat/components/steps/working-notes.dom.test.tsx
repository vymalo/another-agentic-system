// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TurnSteps } from "@/features/chat/lib/step-tree";
import { type ExpansionState, NO_EXPANSION } from "./expansion";
import { StepsPane } from "./steps-pane";
import {
  actorPart,
  assistant,
  RUNNING_VIEW,
  saidPart,
  statusPart,
  stepPart,
  turnsOf,
} from "./testing";
import { TurnSummary } from "./turn-summary";

/*
 * What the agent said while it worked (ADR 0031): rows among the steps of the panel, and, while the
 * turn runs, one quiet line under the turn's line in the chat.
 */

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

function Pane({ turns, live = false }: { turns: readonly TurnSteps[]; live?: boolean }) {
  const [expanded, setExpanded] = useState<ExpansionState>(NO_EXPANSION);
  return (
    <StepsPane
      turns={turns}
      focus={null}
      live={live}
      expanded={expanded}
      onExpandedChange={setExpanded}
    />
  );
}

const LONG = `${"The tests fail because Node 24 wants an explicit glob for the test directory. ".repeat(4)}`;

const turn = (running = false): TurnSteps[] =>
  turnsOf(
    [
      assistant(
        [
          actorPart(),
          statusPart("working", undefined, 1),
          ...saidPart("I'll build something small first.", "working"),
          stepPart("a", "completed", { label: "write_file" }, 2),
          ...saidPart(LONG, "working"),
          stepPart("b", "completed", { label: "run_checks" }, 3),
          ...(running ? [] : saidPart("It is done.", "answer")),
        ],
        running ? { type: "running" } : { type: "complete" },
      ),
    ],
    running ? RUNNING_VIEW : undefined,
  );

const notes = () =>
  [...document.querySelectorAll<HTMLElement>('li[data-kind="note"]')].map((li) => li);

describe("the notes in the panel", () => {
  it("are rows among the steps, in the order things happened, and the answer is not one", () => {
    render(<Pane turns={turn()} />);
    const rows = [...document.querySelectorAll<HTMLElement>("li[data-slot=step]")].map(
      (li) => li.dataset.kind ?? "status",
    );
    expect(rows).toEqual(["status", "note", "tool", "note", "tool"]);
    expect(notes()).toHaveLength(2);
    expect(screen.queryByText("It is done.")).toBeNull();
  });

  it("say what they are to a screen reader before their words, and hold the words whole", () => {
    render(<Pane turns={turn()} />);
    const [first, second] = notes();
    expect(first?.textContent).toContain("Working note: I'll build something small first.");
    // a long note is clamped for the eye, never cut in the page
    expect(second?.textContent).toContain(LONG.trim());
    const text = second?.querySelector('[data-slot="note-text"]') as HTMLElement;
    expect(text.className).toContain("line-clamp-3");
  });

  it("a long note has a control for the rest; a short one has none", () => {
    render(<Pane turns={turn()} />);
    const [first, second] = notes();
    expect(within(first as HTMLElement).queryByRole("button")).toBeNull();
    const more = within(second as HTMLElement).getByRole("button", { name: "Show more" });
    expect(more.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(more);
    expect(more.getAttribute("aria-expanded")).toBe("true");
    expect(more.textContent).toBe("Show less");
    expect(
      (second as HTMLElement).querySelector('[data-slot="note-text"]')?.className,
    ).not.toContain("line-clamp-3");
  });

  it("are drawn as text, never as markup", () => {
    const turns = turnsOf([
      assistant([
        actorPart(),
        ...saidPart("<img src=x onerror=alert(1)> and **bold**", "working"),
        stepPart("a", "completed"),
        ...saidPart("End.", "answer"),
      ]),
    ]);
    render(<Pane turns={turns} />);
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("strong")).toBeNull();
    expect(notes()[0]?.textContent).toContain("<img src=x onerror=alert(1)> and **bold**");
  });

  it("make a turn that said things while it worked a section of the panel, even with no step", () => {
    const turns = turnsOf([
      assistant([
        actorPart(),
        ...saidPart("Just thinking aloud.", "working"),
        ...saidPart("OK.", "answer"),
      ]),
    ]);
    render(<Pane turns={turns} />);
    expect(screen.getByRole("heading", { level: 3 }).textContent).toContain("Turn 1");
    expect(notes()).toHaveLength(1);
  });

  it("a turn that is only its answer is not listed", () => {
    render(<Pane turns={turnsOf([assistant([actorPart(), ...saidPart("Hello.")])])} />);
    expect(screen.queryAllByRole("heading", { level: 3 })).toHaveLength(0);
  });
});

describe("the ticker in the turn's line", () => {
  const show = (turns: TurnSteps[]) =>
    render(<TurnSummary turn={turns[0] as TurnSteps} panelShowsTurn={false} onOpen={() => {}} />);
  const ticker = () => document.querySelector<HTMLElement>('[data-slot="turn-ticker"]');

  it("shows the last working sentence while the turn runs, quietly, in muted text", () => {
    const turns = turnsOf(
      [
        assistant(
          [
            actorPart(),
            ...saidPart("I'll run the tests.", "working"),
            stepPart("a", "completed", { label: "write_file" }, 2),
            ...saidPart("Fixing the glob.\nThen running them again.", "working"),
          ],
          { type: "running" },
        ),
      ],
      RUNNING_VIEW,
    );
    show(turns);
    expect(ticker()?.textContent).toBe("Then running them again.");
    expect(ticker()?.className).toContain("text-muted-foreground");
  });

  it("is one line that gives way to an ellipsis, and never longer than the page cuts it", () => {
    show(turn(true));
    const text = ticker()?.textContent ?? "";
    expect(ticker()?.className).toContain("truncate");
    expect(text.length).toBeLessThanOrEqual(160);
    expect(text.endsWith("…")).toBe(true);
  });

  it("is not a live region: it does not talk over the state pill and the reply", () => {
    show(turn(true));
    const line = ticker() as HTMLElement;
    expect(line.closest("[aria-live]")).toBeNull();
    expect(line.closest("[role=status],[role=alert],[role=log]")).toBeNull();
    expect(line.hasAttribute("aria-live")).toBe(false);
  });

  it("does not change what the line's button is called, and does not take the focus", () => {
    show(turn(true));
    const button = screen.getByRole("button", { name: /steps:/ });
    expect(button.getAttribute("aria-label")).toBe(
      "Coder's steps: run_checks · 3 steps. Show in the side panel",
    );
    expect(ticker()?.querySelector("button,a,[tabindex]")).toBeNull();
    expect(document.activeElement).toBe(document.body);
  });

  it("is gone when the turn is over", () => {
    show(turn(false));
    expect(ticker()).toBeNull();
  });
});
