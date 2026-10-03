// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TurnSteps } from "@/features/chat/lib/step-tree";
import { type ExpansionState, NO_EXPANSION } from "./expansion";
import { StepsPane } from "./steps-pane";
import {
  actorPart,
  askPart,
  assistant,
  RUNNING_VIEW,
  statusPart,
  stepPart,
  turnsOf,
} from "./testing";

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

function Harness({ turns, live = false }: { turns: readonly TurnSteps[]; live?: boolean }) {
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

const turnOf = (parts: ReturnType<typeof askPart>[], done = true) =>
  turnsOf(
    [
      assistant(
        [actorPart(), statusPart("working"), ...parts],
        done ? { type: "complete" } : { type: "running" },
      ),
    ],
    done ? undefined : RUNNING_VIEW,
  );
const lineOf = (name: RegExp | string) => screen.getByRole("button", { name });
const rowOf = (el: HTMLElement) => el.closest("li") as HTMLElement;

describe("every end state of an ask says itself in words", () => {
  const CASES: [string, string, Record<string, unknown>][] = [
    ["completed", "Answered", {}],
    ["input_required", "Asked back", { question: "Which branch?" }],
    ["auth_required", "Needs sign-in", { question: "Sign in at the portal first" }],
    ["failed", "Failed", { error: "the agent crashed" }],
    ["rejected", "Refused", { error: "the agent does not take this work" }],
    ["canceled", "Stopped", { error: "the asking task ended" }],
    ["timed_out", "Timed out", { error: "no answer in 1800 s" }],
  ];
  for (const [state, word, extra] of CASES) {
    it(`${state}: "${word}" on the line${extra.question || extra.error ? ", and why under it" : ""}`, () => {
      render(<Harness turns={turnOf([askPart(1, "coder", state, extra)])} />);
      const line = lineOf(/^Asked coder/);
      expect(line.textContent).toBe(`Asked coder: ${word}`);
      expect(line.getAttribute("aria-expanded")).toBe("false");
      const row = rowOf(line);
      expect(row.getAttribute("data-ask-state")).toBe(state);
      const note = row.querySelector('[data-slot="ask-note"]');
      const said = (extra.question ?? extra.error) as string | undefined;
      if (said) expect(note?.textContent).toContain(said);
      else expect(note).toBeNull();
    });
  }

  it("a question back is a question, and a failure is a failure: not the same words", () => {
    render(
      <Harness
        turns={turnOf([
          askPart(1, "coder", "input_required", { question: "Which branch?" }),
          askPart(2, "reviewer", "failed", { error: "boom" }, 2),
        ])}
      />,
    );
    expect(rowOf(lineOf(/^Asked coder/)).querySelector('[data-slot="ask-note"]')?.textContent).toBe(
      "It asks: Which branch?",
    );
    expect(
      rowOf(lineOf(/^Asked reviewer/)).querySelector('[data-slot="ask-note"]')?.textContent,
    ).toBe("Why: boom");
    // only the failure is a failure of the turn
    expect(screen.getAllByText("1 failed").length).toBeGreaterThan(0);
  });
});

describe("a failure is visible at every level", () => {
  const nested = () =>
    turnOf([
      askPart(1, "coder", "completed", { answer: "done" }),
      askPart(2, "researcher", "failed", { by: "ask:1", depth: 2, error: "scripted failure" }, 2),
    ]);

  it("a completed ask above a failed one says so with its chip while it is closed", () => {
    render(<Harness turns={nested()} />);
    const parent = lineOf(/^Asked coder/);
    expect(parent.textContent).toBe("Asked coder: Answered· 1 step");
    const chip = within(rowOf(parent)).getByText("1 failed");
    expect(chip.closest('[data-slot="failed-chip"]')).not.toBeNull();
    // the child's own line is not on the page until the parent opens
    expect(screen.queryByRole("button", { name: /^Asked researcher/ })).toBeNull();
  });

  it("opened, the parent lists the child with its own word and its reason, and the chip gives way", () => {
    render(<Harness turns={nested()} />);
    const parent = lineOf(/^Asked coder/);
    fireEvent.click(parent);
    const child = lineOf(/^Asked researcher/);
    expect(child.textContent).toBe("Asked researcher: Failed");
    const row = rowOf(child);
    expect(row.getAttribute("data-state")).toBe("failed");
    expect(row.querySelector('[data-slot="ask-note"]')?.textContent).toBe("Why: scripted failure");
    // the parent's chip is for what is below a closed line; the child is the failure now
    expect(within(rowOf(parent)).queryAllByText("1 failed")).toHaveLength(0);
  });

  it("the turn's header says it, whatever is open", () => {
    render(<Harness turns={nested()} />);
    const heading = screen.getByRole("heading", { level: 3 });
    expect(within(heading).getByText("1 failed")).toBeTruthy();
  });
});

describe("an ask is a disclosure", () => {
  const parts = () => [
    askPart(1, "coder", "completed", {
      answer: "Pictures are in the folder.",
      artifacts: [
        { name: "pitch.png", uri: "https://example.org/pitch.png" },
        { name: "evil", uri: "javascript:alert(1)" },
        { name: "kept" },
      ],
    }),
    stepPart("tool-9", "completed", { label: "Web search", icon: "web" }, 3, ["ask-1"]),
  ];

  it("opens with Enter and Space on its button, onto what was asked, answered and handed back, and its steps", () => {
    render(<Harness turns={turnOf(parts())} />);
    const line = lineOf(/^Asked coder/);
    expect(line.getAttribute("aria-expanded")).toBe("false");
    expect(line.getAttribute("aria-controls")).toBeNull();
    fireEvent.click(line);
    expect(line.getAttribute("aria-expanded")).toBe("true");
    const controls = (line.getAttribute("aria-controls") ?? "").split(" ");
    expect(controls).toHaveLength(2);
    for (const id of controls) expect(document.getElementById(id)).not.toBeNull();
    const row = rowOf(line);
    const details = within(row.querySelector('[data-slot="ask-details"]') as HTMLElement);
    expect(details.getByText("Please help, coder")).toBeTruthy();
    expect(details.getByText("Pictures are in the folder.")).toBeTruthy();
    expect(details.getByRole("link", { name: "pitch.png" }).getAttribute("href")).toBe(
      "https://example.org/pitch.png",
    );
    // a javascript: URI and a name with none are text, not links
    expect(details.queryByRole("link", { name: "evil" })).toBeNull();
    expect(details.getByText("evil")).toBeTruthy();
    expect(details.getByText("kept")).toBeTruthy();
    // the steps it took sit under it
    expect(within(row).getByRole("list", { name: "Steps of Asked coder" })).toBeTruthy();
    expect(within(row).getByText("Web search")).toBeTruthy();
    fireEvent.click(line);
    expect(line.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("Pictures are in the folder.")).toBeNull();
    expect(screen.queryByText("Web search")).toBeNull();
  });

  it("is a real button: a key press is its click, and the focus stays on it", () => {
    render(<Harness turns={turnOf(parts())} />);
    const line = lineOf(/^Asked coder/);
    expect(line.tagName).toBe("BUTTON");
    expect(line.getAttribute("type")).toBe("button");
    line.focus();
    expect(document.activeElement).toBe(line);
  });

  it("draws an agent's words as text: markup in them is not markup", () => {
    render(
      <Harness
        turns={turnOf([
          askPart(1, "coder", "failed", {
            text: "<img src=x onerror=alert(1)> ask",
            error: "<b>bold</b> failure",
          }),
        ])}
      />,
    );
    const line = lineOf(/^Asked coder/);
    expect(screen.getByText(/<b>bold<\/b> failure/)).toBeTruthy();
    fireEvent.click(line);
    expect(screen.getByText("<img src=x onerror=alert(1)> ask")).toBeTruthy();
    expect(document.querySelector("img[src='x']")).toBeNull();
    expect(document.querySelector("b")).toBeNull();
  });

  it("an ask that runs has a spinner, and a turn that is no longer running stops it", () => {
    const running = turnOf([askPart(1, "coder", "running")], false);
    const { unmount } = render(<Harness turns={running} live />);
    const row = rowOf(lineOf(/^Asked coder/));
    expect(row.getAttribute("data-state")).toBe("live");
    expect(row.querySelector("svg.lucide-loader-circle")).not.toBeNull();
    unmount();
    render(<Harness turns={turnOf([askPart(1, "coder", "running")])} />);
    const stopped = rowOf(lineOf(/^Asked coder/));
    expect(stopped.getAttribute("data-state")).toBe("muted");
    expect(stopped.querySelector("svg.lucide-loader-circle")).toBeNull();
    // the log never ended it: its words are the turn's, not "Working"
    expect(lineOf(/^Asked coder/).textContent).toBe("Asked coder: Stopped");
  });
});
