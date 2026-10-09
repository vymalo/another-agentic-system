// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { StateBadge } from "@/features/chat/components/state-badge";
import type { SummaryIcon } from "@/features/chat/lib/step-tree";
import type { ThreadState } from "@/lib/api/types";
import { TurnGlyph } from "./turn-glyph";

afterEach(cleanup);

const shapeOf = (el: Element | null): string | undefined =>
  el?.getAttribute("class")?.match(/lucide-[a-z-]+/)?.[0];

const glyph = (icon: SummaryIcon) => {
  const { container } = render(<TurnGlyph icon={icon} />);
  const shape = shapeOf(container.querySelector("svg"));
  cleanup();
  return shape;
};
const pill = (state: ThreadState) => {
  const { container } = render(<StateBadge state={state} />);
  const shape = shapeOf(container.querySelector("svg"));
  cleanup();
  return shape;
};

describe("a turn's glyph and the thread's state pill", () => {
  it("draw the states they share in the same shapes", () => {
    const shared: [SummaryIcon, ThreadState][] = [
      ["spinner", "working"],
      ["verifying", "verifying"],
      ["check", "done"],
      ["cross", "failed"],
      ["stopped", "cancelled"],
    ];
    for (const [icon, state] of shared) {
      expect(glyph(icon), `${icon} / ${state}`).toBe(pill(state));
    }
  });

  it("do not read checking the work as a pass, nor a stop as a ban", () => {
    expect(glyph("verifying")).not.toContain("shield-check");
    expect(glyph("stopped")).not.toContain("ban");
  });

  it("give every glyph a shape of its own", () => {
    const icons: SummaryIcon[] = ["spinner", "check", "cross", "pause", "verifying", "stopped"];
    const shapes = icons.map(glyph);
    expect(shapes.every(Boolean)).toBe(true);
    expect(new Set(shapes).size).toBe(icons.length);
  });
});
