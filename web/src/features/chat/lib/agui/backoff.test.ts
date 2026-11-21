import { describe, expect, it } from "vitest";
import { BACKOFF_CAP_MS, backoffMs } from "./backoff";

/** A draw that always says `n`: the low end of the window (0) or its high end (just under 1). */
const draw = (n: number) => () => n;

describe("backoffMs", () => {
  // ADR 0082, C3: "a growing, randomised wait". Growth is what gets a client past a restart; the draw
  // is what keeps a fleet that one proxy cut at one instant from coming back in one wave.
  it("grows with the attempt and stops at the cap", () => {
    const top = (attempt: number) => backoffMs(attempt, draw(1));
    expect(top(0)).toBe(1000);
    expect(top(1)).toBe(2000);
    expect(top(3)).toBe(8000);
    expect(top(10)).toBe(BACKOFF_CAP_MS);
    // a long outage does not make the arithmetic overflow into Infinity or NaN
    expect(top(400)).toBe(BACKOFF_CAP_MS);
  });

  it("never waits longer than the cap, whatever the draw", () => {
    for (let attempt = 0; attempt < 40; attempt++) {
      for (const r of [0, 0.5, 0.999])
        expect(backoffMs(attempt, draw(r))).toBeLessThanOrEqual(BACKOFF_CAP_MS);
    }
  });

  it("two clients cut at the same instant do not sleep the same amount", () => {
    // same attempt, two draws: the waits differ, and neither leaves the window
    const first = backoffMs(2, draw(0.1));
    const second = backoffMs(2, draw(0.9));
    expect(first).not.toBe(second);
    expect(first).toBe(400);
    expect(second).toBe(3600);
    // and the window it draws from is the one the attempt allows
    expect(Math.min(first, second)).toBeGreaterThanOrEqual(0);
    expect(Math.max(first, second)).toBeLessThan(4000);
  });

  it("draws for real when nothing is injected: the wait is not a constant", () => {
    const waits = new Set(Array.from({ length: 16 }, () => backoffMs(6)));
    expect(waits.size).toBeGreaterThan(1);
  });
});
