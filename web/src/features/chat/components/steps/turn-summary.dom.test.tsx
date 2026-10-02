// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ACTIVITY } from "@/features/chat/lib/agui/vymalo";
import type { TurnSteps } from "@/features/chat/lib/step-tree";
import {
  AT,
  assistant,
  openCodeTurn,
  part,
  RUNNING_VIEW,
  statusPart,
  stepPart,
  textPart,
  turnsOf,
} from "./testing";
import { TurnSummary } from "./turn-summary";

afterEach(cleanup);

const first = (turns: TurnSteps[]): TurnSteps => turns[0] as TurnSteps;
const show = (
  turn: TurnSteps,
  props: { shows?: boolean; onOpen?: (id: string, stepId?: string) => void } = {},
) =>
  render(
    <TurnSummary
      turn={turn}
      panelShowsTurn={props.shows ?? false}
      onOpen={props.onOpen ?? (() => {})}
    />,
  );
const summary = () => screen.getByRole("button", { name: /steps:/ });
const line = () => summary().closest('[data-slot="turn-summary-line"]') as HTMLElement;

describe("the turn's one line", () => {
  it("is a button that says the steps it opens and where: its name tells it all", () => {
    show(first(turnsOf([openCodeTurn(2, { failAt: 1 })])));
    // "Started working", the sub-agent and its two children; one of them failed
    expect(summary().getAttribute("aria-label")).toBe(
      "Coder's steps: 4 steps · 3s, 1 failed. Show in the side panel",
    );
    expect(summary().getAttribute("aria-controls")).toBe("thread-panel");
    expect(summary().getAttribute("data-slot")).toBe("turn-summary");
    expect(summary().tagName).toBe("BUTTON");
    expect(summary().getAttribute("type")).toBe("button");
  });

  it("says what a running turn is on, with a spinner that only a running turn has", () => {
    const running = first(turnsOf([openCodeTurn(2, { running: true })], RUNNING_VIEW));
    show(running);
    // the deepest step that runs is the sub-agent: its children are done
    expect(within(summary()).getByText("OpenCode · 4 steps")).toBeTruthy();
    expect(summary().querySelector('[data-glyph="spinner"]')).not.toBeNull();
    cleanup();
    show(first(turnsOf([openCodeTurn(2)])));
    expect(summary().querySelector('[data-glyph="spinner"]')).toBeNull();
    expect(summary().querySelector('[data-glyph="check"]')).not.toBeNull();
  });

  it("names the step it is on", () => {
    const parts = [
      statusPart("working", undefined, 1),
      stepPart("T/c", "running", { kind: "command", label: "npm test", icon: "execute" }, 2),
    ];
    show(first(turnsOf([assistant(parts, { type: "running" })], RUNNING_VIEW)));
    expect(within(summary()).getByText("Running npm test · 2 steps")).toBeTruthy();
    expect(summary().getAttribute("aria-label")).toBe(
      "Coder's steps: Running npm test · 2 steps. Show in the side panel",
    );
  });

  it("says a failed step even when the turn went well, with an icon and the words", () => {
    show(first(turnsOf([openCodeTurn(3, { failAt: 0 })])));
    const chip = within(line()).getByText("1 failed");
    expect(chip.closest('[data-slot="failed-chip"]')).not.toBeNull();
    expect(chip.closest('[data-slot="failed-chip"]')?.querySelector("svg")).not.toBeNull();
    expect(summary().querySelector('[data-glyph="check"]')).not.toBeNull();
  });

  it("has the failed chip as a button of its own, beside the line's, that opens the first failed step", () => {
    const onOpen = vi.fn();
    const turns = turnsOf([openCodeTurn(3, { id: "turn-x", failAt: 1 })]);
    show(first(turns), { onOpen });
    const chip = screen.getByRole("button", { name: /^1 failed\. Show the first one/ });
    // beside the line's button, not inside it: a button does not hold a button
    expect(summary().contains(chip)).toBe(false);
    expect(chip.closest('[data-slot="turn-summary-line"]')).toBe(line());
    expect(chip.getAttribute("aria-label")).not.toMatch(/message/i);
    fireEvent.click(chip);
    expect(onOpen).toHaveBeenCalledExactlyOnceWith("turn-x", "T/c1");
    onOpen.mockClear();
    fireEvent.click(summary());
    expect(onOpen).toHaveBeenCalledExactlyOnceWith("turn-x");
  });

  it("counts a failed run_checks once, not as its step and its checks artifact", () => {
    const parts = [
      statusPart("working", undefined, 1),
      stepPart("T/rc", "running", { label: "run_checks" }, 2),
      part(ACTIVITY.artifact, { kind: "checks", name: "checks", passed: false, at: AT(3) }),
      stepPart("T/rc", "failed", { label: "run_checks" }, 4),
    ];
    show(first(turnsOf([assistant(parts)])));
    expect(summary().getAttribute("aria-label")).toBe(
      "Coder's steps: 3 steps · 3s, 1 failed. Show in the side panel",
    );
    expect(within(line()).getByText("1 failed")).toBeTruthy();
  });

  it("says no failure when there is none", () => {
    show(first(turnsOf([openCodeTurn(3)])));
    expect(screen.queryByText(/failed/)).toBeNull();
    expect(summary().getAttribute("aria-label")).not.toContain("failed");
  });

  it("says paused for a turn that asked, verifying while the gate checks, failed for a failed one", () => {
    const parts = [statusPart("working"), stepPart("T/a", "completed")];
    show(first(turnsOf([assistant(parts, { type: "requires-action", reason: "interrupt" })])));
    expect(within(summary()).getByText("Paused · 2 steps")).toBeTruthy();
    expect(summary().querySelector('[data-glyph="pause"]')).not.toBeNull();
    cleanup();

    show(
      first(
        turnsOf([assistant(parts, { type: "running" })], {
          state: "verifying",
          waiting: false,
          agentId: "coder",
        }),
      ),
    );
    expect(within(summary()).getByText("Verifying")).toBeTruthy();
    expect(summary().querySelector('[data-glyph="verifying"]')).not.toBeNull();
    cleanup();

    show(first(turnsOf([assistant(parts, { type: "incomplete", reason: "error" })])));
    expect(within(summary()).getByText("Failed · 2 steps")).toBeTruthy();
    expect(summary().querySelector('[data-glyph="cross"]')).not.toBeNull();
    cleanup();

    show(first(turnsOf([assistant(parts, { type: "incomplete", reason: "cancelled" })])));
    expect(within(summary()).getByText("Stopped · 2 steps")).toBeTruthy();
  });

  it("draws nothing for a turn of words only", () => {
    const { container } = show(first(turnsOf([assistant([textPart("just words")])])));
    expect(container.firstChild).toBeNull();
  });

  it("opens the panel on its turn when clicked", () => {
    const onOpen = vi.fn();
    const turns = turnsOf([openCodeTurn(2, { id: "turn-x" })]);
    show(first(turns), { onOpen });
    fireEvent.click(summary());
    expect(onOpen).toHaveBeenCalledExactlyOnceWith("turn-x");
  });

  it("says whether the panel shows this turn, and not only when it does", () => {
    const turn = first(turnsOf([openCodeTurn(2)]));
    const { rerender } = show(turn, { shows: false });
    expect(summary().getAttribute("aria-expanded")).toBe("false");
    rerender(<TurnSummary turn={turn} panelShowsTurn={true} onOpen={() => {}} />);
    expect(summary().getAttribute("aria-expanded")).toBe("true");
  });
});
