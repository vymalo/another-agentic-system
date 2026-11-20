// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ThreadViewProvider } from "../thread-view";
import {
  ANNOUNCE_AFTER_MS,
  ANNOUNCE_MAX_WAIT_MS,
  ANNOUNCE_SETTLE_MS,
  forgetPlans,
  PlanAnnouncer,
} from "./plan-announcer";

/*
 * The announcer on its own (the plan in the conversation is tested with the transcript, plan.dom.test.tsx): what
 * the page remembers of a plan lives outside the component, keyed by the thread and the plan's surface id, because
 * the message that draws a plan is drawn again when its run ends, and a change that was being waited for must
 * not be lost with it.
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
const quiet = (ms = ANNOUNCE_AFTER_MS + 10) => act(() => void vi.advanceTimersByTime(ms));
const said = (c: HTMLElement) =>
  [...(region(c)?.querySelectorAll("p") ?? [])].map((p) => p.textContent);

/** The first drawing of a plan, and the time it takes to settle: what follows is a change. */
const start = (ui: ReactNode) => {
  const v = render(ui);
  act(() => void vi.advanceTimersByTime(ANNOUNCE_SETTLE_MS));
  return v;
};

function Plan({
  thread = "t1",
  surfaceId = "plan-a",
  signature,
  progress,
}: {
  thread?: string;
  surfaceId?: string;
  signature: string;
  progress?: { done: number; total: number };
}): ReactNode {
  return (
    <ThreadViewProvider
      value={{ state: undefined, waiting: false, agentId: null, threadId: thread }}
    >
      <PlanAnnouncer surfaceId={surfaceId} signature={signature} progress={progress} />
    </ThreadViewProvider>
  );
}

describe("PlanAnnouncer", () => {
  it("says nothing for the first copy, then the change once it has been quiet", () => {
    const view = start(<Plan signature="one" progress={{ done: 0, total: 2 }} />);
    quiet();
    expect(said(view.container)).toEqual([]);
    view.rerender(<Plan signature="two" progress={{ done: 1, total: 2 }} />);
    expect(said(view.container)).toEqual([]);
    quiet();
    expect(said(view.container)).toEqual(["Plan updated: 1 of 2 done"]);
  });

  it("says a plan with no checklist plainly", () => {
    const view = start(<Plan signature="one" />);
    view.rerender(<Plan signature="two" />);
    quiet();
    expect(said(view.container)).toEqual(["Plan updated"]);
  });

  it("two changes 400 ms apart are one announcement, after 0.8 s of quiet from the last", () => {
    const view = start(<Plan signature="one" progress={{ done: 0, total: 3 }} />);
    view.rerender(<Plan signature="two" progress={{ done: 1, total: 3 }} />);
    quiet(400);
    view.rerender(<Plan signature="three" progress={{ done: 2, total: 3 }} />);
    // 0.8 s after the FIRST change is not quiet yet: the second one moved the wait
    quiet(500);
    expect(said(view.container)).toEqual([]);
    quiet(400);
    expect(said(view.container)).toEqual(["Plan updated: 2 of 3 done"]);
  });

  it("a plan updated more often than every 0.8 s is still announced, at the latest after the maximum wait", () => {
    const view = start(<Plan signature="s0" progress={{ done: 0, total: 100 }} />);
    let n = 0;
    // a change every 400 ms for ten seconds: never quiet for 0.8 s
    for (let t = 0; t < 10_000; t += 400) {
      n += 1;
      view.rerender(<Plan signature={`s${n}`} progress={{ done: n, total: 100 }} />);
      quiet(400);
    }
    const lines = said(view.container);
    expect(lines.length).toBeGreaterThanOrEqual(1);
    expect(lines.length).toBeLessThanOrEqual(Math.ceil(10_000 / ANNOUNCE_MAX_WAIT_MS) + 1);
    // the first one came by the maximum, not by a quiet that never was
    const first = start(<Plan thread="t2" signature="a" />);
    first.rerender(<Plan thread="t2" signature="b" />);
    quiet(ANNOUNCE_MAX_WAIT_MS);
    expect(said(first.container)).toEqual(["Plan updated"]);
  });

  it("a change that is waited for survives the message being drawn again", () => {
    const first = start(<Plan signature="one" progress={{ done: 0, total: 2 }} />);
    first.rerender(<Plan signature="two" progress={{ done: 1, total: 2 }} />);
    // the run ends: the message is drawn again, and the old one goes away with its timer's owner
    first.unmount();
    const again = render(<Plan signature="two" progress={{ done: 1, total: 2 }} />);
    expect(said(again.container)).toEqual([]);
    quiet();
    expect(said(again.container)).toEqual(["Plan updated: 1 of 2 done"]);
  });

  it("a plan drawn again unchanged is not a change", () => {
    const first = start(<Plan signature="one" progress={{ done: 0, total: 1 }} />);
    first.unmount();
    const again = start(<Plan signature="one" progress={{ done: 0, total: 1 }} />);
    quiet();
    expect(said(again.container)).toEqual([]);
  });

  it("another plan of the thread has its own memory", () => {
    const a = start(<Plan surfaceId="plan-a" signature="one" />);
    const b = start(<Plan surfaceId="plan-b" signature="other" />);
    quiet();
    expect(said(a.container)).toEqual([]);
    expect(said(b.container)).toEqual([]);
  });

  it("another thread has its own memory: its first copy is not announced and its first update is", () => {
    // thread A has a plan; the person moves to thread B, whose plan has the very same id
    const a = start(<Plan thread="thread-a" signature="from-a" />);
    a.unmount();
    const b = start(<Plan thread="thread-b" signature="from-b" />);
    quiet();
    expect(said(b.container)).toEqual([]);
    b.rerender(
      <Plan thread="thread-b" signature="from-b-again" progress={{ done: 1, total: 2 }} />,
    );
    quiet();
    expect(said(b.container)).toEqual(["Plan updated: 1 of 2 done"]);
  });

  it("a change in the first second after a plan is drawn is the plan arriving, not changing", () => {
    const view = render(<Plan signature="one" progress={{ done: 0, total: 2 }} />);
    act(() => void vi.advanceTimersByTime(ANNOUNCE_SETTLE_MS - 100));
    view.rerender(<Plan signature="two" progress={{ done: 1, total: 2 }} />);
    quiet();
    expect(said(view.container)).toEqual([]);
  });

  it("is a polite status region that is always in the page", () => {
    const view = start(<Plan signature="one" />);
    const r = region(view.container);
    expect(r?.getAttribute("role")).toBe("status");
    expect(r?.getAttribute("aria-live")).toBe("polite");
  });
});
