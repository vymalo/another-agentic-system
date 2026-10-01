import { describe, expect, it } from "vitest";
import { groupByRecency, recencyOf } from "./recency";

const NOW = new Date(2026, 8, 30, 15, 0, 0); // 30 Sep 2026, 15:00 local
const at = (days: number, hours = 12) => new Date(2026, 8, 30 - days, hours, 0, 0).toISOString();

describe("the thread list's groups", () => {
  it("puts a thread in the group of the local day it was last touched", () => {
    expect(recencyOf(at(0, 0), NOW)).toBe("Today");
    expect(recencyOf(at(1, 23), NOW)).toBe("Yesterday");
    expect(recencyOf(at(1, 0), NOW)).toBe("Yesterday");
    expect(recencyOf(at(2), NOW)).toBe("Previous 7 days");
    expect(recencyOf(at(7, 0), NOW)).toBe("Previous 7 days");
    expect(recencyOf(at(8), NOW)).toBe("Previous 30 days");
    expect(recencyOf(at(40), NOW)).toBe("Older");
  });

  it("reads a time it cannot parse as old, and one from the future as today", () => {
    expect(recencyOf("not a time", NOW)).toBe("Older");
    expect(recencyOf(at(-2), NOW)).toBe("Today");
  });

  it("keeps the order of the list inside a group and leaves out the empty groups", () => {
    const items = [
      { id: "a", updatedAt: at(0, 14) },
      { id: "b", updatedAt: at(0, 9) },
      { id: "c", updatedAt: at(3) },
      { id: "d", updatedAt: at(90) },
    ];
    expect(groupByRecency(items, NOW)).toEqual([
      { group: "Today", items: [items[0], items[1]] },
      { group: "Previous 7 days", items: [items[2]] },
      { group: "Older", items: [items[3]] },
    ]);
  });
});

describe("the groups across a change of clocks", () => {
  it("yesterday is the calendar day, even when it had 25 hours", () => {
    const tz = process.env.TZ;
    // Europe/Berlin leaves summer time on 25 Oct 2026: that Sunday has 25 hours
    process.env.TZ = "Europe/Berlin";
    try {
      const now = new Date(2026, 9, 26, 0, 30); // Monday 00:30
      // Sunday 00:30: the first hour of Yesterday, 24 hours and one before now
      const early = new Date(2026, 9, 25, 0, 30);
      expect(now.getTime() - early.getTime()).toBe(25 * 60 * 60 * 1000);
      expect(recencyOf(early.toISOString(), now)).toBe("Yesterday");
      expect(recencyOf(new Date(2026, 9, 24, 23, 59).toISOString(), now)).toBe("Previous 7 days");
    } finally {
      process.env.TZ = tz;
    }
  });
});
