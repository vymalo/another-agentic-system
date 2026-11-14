// @vitest-environment jsdom
import { act, cleanup, render, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ANNOUNCE_AFTER_MS,
  forgetPlans,
  PlanAnnouncer,
  useForgetPlansOnThreadChange,
} from "./plan-announcer";

/*
 * The announcer on its own (the plan in the conversation is tested with the transcript, plan.dom.test.tsx): what
 * the page remembers of a plan lives outside the component, because the message that draws a plan is drawn
 * again when its run ends, and a change that was being waited for must not be lost with it.
 */

beforeEach(() => {
  vi.useFakeTimers();
  forgetPlans();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const region = (c: HTMLElement) => c.querySelector('[data-slot="plan-status"]');
const quiet = () => act(() => void vi.advanceTimersByTime(ANNOUNCE_AFTER_MS + 10));

describe("PlanAnnouncer", () => {
  it("says nothing for the first copy, then the change once it has been quiet", () => {
    const view = render(
      <PlanAnnouncer planKey="a2ui-3" signature="one" progress={{ done: 0, total: 2 }} />,
    );
    quiet();
    expect(region(view.container)?.textContent).toBe("");
    view.rerender(
      <PlanAnnouncer planKey="a2ui-3" signature="two" progress={{ done: 1, total: 2 }} />,
    );
    expect(region(view.container)?.textContent).toBe("");
    quiet();
    expect(region(view.container)?.textContent).toBe("Plan updated: 1 of 2 done");
  });

  it("says a plan with no checklist plainly", () => {
    const view = render(<PlanAnnouncer planKey="a2ui-3" signature="one" progress={undefined} />);
    view.rerender(<PlanAnnouncer planKey="a2ui-3" signature="two" progress={undefined} />);
    quiet();
    expect(region(view.container)?.textContent).toBe("Plan updated");
  });

  it("a change that is waited for survives the message being drawn again", () => {
    const first = render(
      <PlanAnnouncer planKey="a2ui-3" signature="one" progress={{ done: 0, total: 2 }} />,
    );
    first.rerender(
      <PlanAnnouncer planKey="a2ui-3" signature="two" progress={{ done: 1, total: 2 }} />,
    );
    // the run ends: the message is drawn again, and the old one goes away with its timer's owner
    first.unmount();
    const again = render(
      <PlanAnnouncer planKey="a2ui-3" signature="two" progress={{ done: 1, total: 2 }} />,
    );
    expect(region(again.container)?.textContent).toBe("");
    quiet();
    expect(region(again.container)?.textContent).toBe("Plan updated: 1 of 2 done");
  });

  it("a plan drawn again unchanged is not a change", () => {
    const first = render(
      <PlanAnnouncer planKey="a2ui-3" signature="one" progress={{ done: 0, total: 1 }} />,
    );
    first.unmount();
    const again = render(
      <PlanAnnouncer planKey="a2ui-3" signature="one" progress={{ done: 0, total: 1 }} />,
    );
    quiet();
    expect(region(again.container)?.textContent).toBe("");
  });

  it("two plans do not meet: another message has its own memory", () => {
    const a = render(<PlanAnnouncer planKey="a2ui-3" signature="one" progress={undefined} />);
    const b = render(<PlanAnnouncer planKey="a2ui-9" signature="other" progress={undefined} />);
    quiet();
    expect(region(a.container)?.textContent).toBe("");
    expect(region(b.container)?.textContent).toBe("");
  });

  it("is a polite status region that is always in the page", () => {
    const view = render(<PlanAnnouncer planKey="a2ui-3" signature="one" progress={undefined} />);
    const r = region(view.container);
    expect(r?.getAttribute("role")).toBe("status");
    expect(r?.getAttribute("aria-live")).toBe("polite");
  });
});

describe("useForgetPlansOnThreadChange", () => {
  it("drops what is remembered of a plan when the page moves to another thread, and not when a new chat gets its thread", () => {
    const hook = renderHook(({ id }) => useForgetPlansOnThreadChange(id), {
      initialProps: { id: null as string | null },
    });
    const view = render(<PlanAnnouncer planKey="a2ui-3" signature="one" progress={undefined} />);
    // a new chat becomes thread A: the same conversation, nothing is forgotten
    hook.rerender({ id: "thread-a" });
    view.rerender(<PlanAnnouncer planKey="a2ui-3" signature="two" progress={undefined} />);
    quiet();
    expect(region(view.container)?.textContent).toBe("Plan updated");
    // thread B has its own a2ui-3: its first copy is not a change from thread A's
    view.unmount();
    hook.rerender({ id: "thread-b" });
    const other = render(
      <PlanAnnouncer planKey="a2ui-3" signature="elsewhere" progress={undefined} />,
    );
    quiet();
    expect(region(other.container)?.textContent).toBe("");
  });
});
