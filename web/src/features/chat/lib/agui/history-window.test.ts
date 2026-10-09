import { describe, expect, it } from "vitest";
import {
  HistoryGap,
  HistoryWindow,
  type PageMeta,
  ProjectionChanged,
  turnsOfPage,
} from "./history-window";

const config = { initialTurns: 12, pageTurns: 20, maxTurns: 100 };
const page = (start: number, end: number, earlier = start > 1, projection = 1): PageMeta => ({
  start,
  end,
  head: 1000,
  earlier,
  projection,
});

describe("the size of the pages", () => {
  it("grows from pageTurns, doubling, and stops at what the server accepts", () => {
    expect([0, 1, 2, 3, 4, 10].map((n) => turnsOfPage(n, config))).toEqual([
      20, 40, 80, 100, 100, 100,
    ]);
    expect(turnsOfPage(0, { initialTurns: 1, pageTurns: 7, maxTurns: 7 })).toBe(7);
    expect(turnsOfPage(50, config)).toBe(100);
  });
});

describe("the pages held", () => {
  it("starts with the newest page, whose start is where the next older one is asked from", () => {
    const w = new HistoryWindow(page(81, 100), config);
    expect([w.start, w.end, w.earlier, w.before, w.pages]).toEqual([81, 100, true, 81, 1]);
    expect(w.nextTurns).toBe(20);
  });

  it("joins an older page that ends the event before its oldest begins, and asks for more each time", () => {
    const w = new HistoryWindow(page(81, 100), config);
    w.addOlder(page(41, 80));
    expect([w.start, w.before, w.pages, w.nextTurns]).toEqual([41, 41, 2, 40]);
    w.addOlder(page(1, 40, false));
    expect([w.start, w.earlier, w.pages, w.nextTurns]).toEqual([1, false, 3, 80]);
    expect(w.end).toBe(100);
  });

  it("refuses a page that leaves a hole or overlaps, and keeps what it holds", () => {
    const w = new HistoryWindow(page(81, 100), config);
    for (const bad of [page(41, 79), page(41, 81), page(1, 40), page(81, 100), page(82, 81)]) {
      expect(() => w.addOlder(bad)).toThrow(HistoryGap);
    }
    expect([w.start, w.pages]).toEqual([81, 1]);
    w.addOlder(page(41, 80));
    expect(w.pages).toBe(2);
  });

  it("refuses a page written by another projection than the ones held, and keeps what it holds", () => {
    const w = new HistoryWindow(page(81, 100), config);
    expect(() => w.addOlder(page(41, 80, true, 2))).toThrow(ProjectionChanged);
    expect(() => w.check(page(41, 80, true, 2))).toThrow(ProjectionChanged);
    expect([w.start, w.pages]).toEqual([81, 1]);
    w.addOlder(page(41, 80));
    expect(w.pages).toBe(2);
  });

  it("checks without holding: a page that joins is not held until it is added", () => {
    const w = new HistoryWindow(page(81, 100), config);
    w.check(page(41, 80));
    expect([w.start, w.pages]).toEqual([81, 1]);
  });

  it("refuses an empty page, which would be asked for again for ever", () => {
    const w = new HistoryWindow(page(81, 100), config);
    expect(() => w.addOlder(page(81, 80))).toThrow(HistoryGap);
  });

  it("accepts exactly the pages of a log cut in any way, newest first, and no other order", () => {
    // a deterministic generator: the log of 400 events cut in pieces of 1 to 60 events
    let seed = 12345;
    const next = (n: number) => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return 1 + (seed % n);
    };
    for (let round = 0; round < 40; round++) {
      const cuts: PageMeta[] = [];
      let end = 400;
      while (end > 0) {
        const start = Math.max(1, end - next(60) + 1);
        cuts.push(page(start, end));
        end = start - 1;
      }
      const w = new HistoryWindow(cuts[0] as PageMeta, config);
      for (const older of cuts.slice(1)) w.addOlder(older);
      expect([w.start, w.earlier, w.pages]).toEqual([1, false, cuts.length]);
      // out of order: a page two back is a hole
      if (cuts.length > 2) {
        const again = new HistoryWindow(cuts[0] as PageMeta, config);
        expect(() => again.addOlder(cuts[2] as PageMeta)).toThrow(HistoryGap);
      }
    }
  });
});
