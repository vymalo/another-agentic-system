// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ThreadState } from "@/lib/api/types";
import { StateBadge } from "./state-badge";

afterEach(cleanup);

const STATES: ThreadState[] = [
  "queued",
  "working",
  "verifying",
  "blocked",
  "done",
  "failed",
  "cancelled",
];

const pill = () => screen.getByRole("status", { name: /^Thread state:/ });

/** The words a sighted person reads on the pill: what is not visually hidden. */
const visibleWords = (el: HTMLElement) =>
  [...el.querySelectorAll("span")]
    .filter((s) => !s.classList.contains("sr-only"))
    .map((s) => s.textContent)
    .join("");

describe("the state pill", () => {
  it("is an icon for a state that asks nothing of the person, with its words for a screen reader", () => {
    for (const state of [
      "queued",
      "working",
      "verifying",
      "done",
      "failed",
      "cancelled",
    ] as const) {
      render(<StateBadge state={state} />);
      expect(visibleWords(pill()), state).toBe("");
      expect(pill().textContent, state).not.toBe("");
      expect(pill().querySelector("svg"), state).not.toBeNull();
      cleanup();
    }
  });

  it("keeps its words on a thread that waits for the person: it is the state that asks them to act", () => {
    render(<StateBadge state="blocked" needsAnswer />);
    expect(visibleWords(pill())).toBe("Your turn");
    cleanup();
    render(<StateBadge state="blocked" />);
    expect(visibleWords(pill())).toBe("Needs attention");
  });

  it("gives every state a shape of its own, still: a state is never told apart by its colour or its motion alone", () => {
    const shapes = new Map<string, string>();
    const shape = () =>
      pill()
        .querySelector("svg")
        ?.getAttribute("class")
        ?.match(/lucide-[a-z-]+/)?.[0];
    for (const state of STATES) {
      render(<StateBadge state={state} />);
      shapes.set(state, shape() ?? "");
      cleanup();
    }
    render(<StateBadge state="blocked" needsAnswer />);
    shapes.set("your-turn", shape() ?? "");
    expect([...shapes.values()].every(Boolean)).toBe(true);
    expect(new Set(shapes.values()).size).toBe(shapes.size);
  });

  it("does not use the shield that means 'passed' for checking the work, nor a ban for a stopped thread", () => {
    render(<StateBadge state="verifying" />);
    expect(pill().querySelector("svg")?.getAttribute("class")).not.toContain("shield-check");
    cleanup();
    render(<StateBadge state="cancelled" />);
    expect(pill().querySelector("svg")?.getAttribute("class")).not.toContain("ban");
  });

  it("keeps its name: 'Thread state: Done', and the longer words for the verifying state", () => {
    render(<StateBadge state="done" />);
    expect(pill().getAttribute("aria-label")).toBe("Thread state: Done");
    cleanup();
    render(<StateBadge state="verifying" />);
    expect(pill().getAttribute("aria-label")).toBe("Thread state: Checking the agent's work");
  });
});
