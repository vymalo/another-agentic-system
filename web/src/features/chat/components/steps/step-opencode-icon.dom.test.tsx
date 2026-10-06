// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TurnSteps } from "@/features/chat/lib/step-tree";
import { type ExpansionState, NO_EXPANSION } from "./expansion";
import { STEP_ICON, STEP_ICON_TITLE } from "./step-icons";
import { StepsPane } from "./steps-pane";
import { actorPart, assistant, statusPart, stepPart, turnsOf } from "./testing";

/*
 * The `opencode` icon of steps/v1 (ADR 0049): the step that hands work to OpenCode over ACP. OpenCode's
 * own logo is not bundled (its terms are unverified), so the glyph is a terminal in a frame and the
 * step says OpenCode in its tooltip and to a screen reader, whatever its label is.
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

function Harness({ turns }: { turns: readonly TurnSteps[] }) {
  const [expanded, setExpanded] = useState<ExpansionState>(NO_EXPANSION);
  return (
    <StepsPane
      turns={turns}
      focus={null}
      live={false}
      expanded={expanded}
      onExpandedChange={setExpanded}
    />
  );
}

const turnOf = (label: string, icon: string): TurnSteps[] =>
  turnsOf([
    assistant([
      actorPart(),
      statusPart("working", undefined, 1),
      stepPart("T/a", "completed", { label, icon }, 2),
    ]),
  ]);

const rowOf = (id: string): HTMLElement =>
  document.querySelector(`[data-step="${id}"]`) as HTMLElement;

describe("a step with the opencode icon", () => {
  it("has a glyph of its own, called OpenCode", () => {
    expect(STEP_ICON.opencode).toBeDefined();
    expect(STEP_ICON.opencode).not.toBe(STEP_ICON.execute);
    expect(STEP_ICON_TITLE.opencode).toBe("OpenCode");
  });

  it("shows OpenCode as the glyph's tooltip and tells a screen reader, when its label does not", () => {
    render(<Harness turns={turnOf("Implementing the retry loop", "opencode")} />);
    const row = rowOf("T/a");
    expect(row.querySelector('[title="OpenCode"]')).not.toBeNull();
    expect(row.querySelector(".sr-only")?.textContent).toContain("OpenCode");
  });

  it("does not say it twice when the label says it", () => {
    render(<Harness turns={turnOf("OpenCode: implementing the retry loop", "opencode")} />);
    const row = rowOf("T/a");
    expect(row.querySelector('[title="OpenCode"]')).not.toBeNull();
    const hidden = [...row.querySelectorAll(".sr-only")].map((n) => n.textContent ?? "");
    expect(hidden.some((t) => t.startsWith("OpenCode"))).toBe(false);
  });

  it("is not named for any other icon", () => {
    render(<Harness turns={turnOf("npm test", "execute")} />);
    expect(rowOf("T/a").querySelector('[title="OpenCode"]')).toBeNull();
  });
});
