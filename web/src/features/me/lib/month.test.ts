import { describe, expect, it } from "vitest";
import { currentMonth, isMonth, monthLabel, nextMonth, previousMonth } from "./month";

describe("months of the usage page (UTC, YYYY-MM)", () => {
  it("knows the shape the API accepts and nothing else", () => {
    for (const ok of ["2026-10", "0001-01", "9999-12", "2024-02"]) expect(isMonth(ok)).toBe(true);
    for (const bad of ["2026-13", "2026-00", "2026-1", "26-10", "2026-10-01", " 2026-10", "", "x"])
      expect(isMonth(bad), bad).toBe(false);
  });

  it("moves back and forward across a year end", () => {
    expect(previousMonth("2026-03")).toBe("2026-02");
    expect(previousMonth("2026-01")).toBe("2025-12");
    expect(nextMonth("2025-12")).toBe("2026-01");
    expect(nextMonth("2026-10")).toBe("2026-11");
  });

  it("keeps a year four digits wide and stops where the API's range does", () => {
    expect(previousMonth("0002-01")).toBe("0001-12");
    expect(previousMonth("0001-01")).toBeNull();
    expect(nextMonth("9999-12")).toBeNull();
  });

  it("reads the current month in UTC, not in the person's zone", () => {
    expect(currentMonth(new Date("2026-10-31T23:59:59Z"))).toBe("2026-10");
    expect(currentMonth(new Date("2026-11-01T00:00:00Z"))).toBe("2026-11");
    expect(currentMonth(new Date("2027-01-01T00:30:00+05:00"))).toBe("2026-12");
  });

  it("names a month in words", () => {
    expect(monthLabel("2026-10")).toBe("October 2026");
    expect(monthLabel("2026-01")).toBe("January 2026");
  });
});
