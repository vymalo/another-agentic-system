import { afterEach, describe, expect, it, vi } from "vitest";
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

/*
 * The month is a UTC one whatever zone the browser is in. CI runs under TZ=UTC, where a read of the local month
 * cannot be told from the UTC one, so these run in zones a day away from it (`Etc/GMT+12` is UTC-12, `Etc/GMT-14` is UTC+14).
 */
describe("currentMonth is the UTC month in any zone", () => {
  afterEach(() => vi.unstubAllEnvs());

  const inZone = (tz: string, at: string) => {
    vi.stubEnv("TZ", tz);
    const now = new Date(at);
    return now;
  };

  it("the zone under test is really applied (the guard of the cases below)", () => {
    const now = inZone("Etc/GMT+12", "2026-12-01T11:30:00Z");
    expect(now.getMonth()).toBe(10); // November, locally
    expect(now.getUTCMonth()).toBe(11);
  });

  it("2026-11-30T23:30 at UTC-12 is already December in UTC", () => {
    expect(currentMonth(inZone("Etc/GMT+12", "2026-11-30T23:30:00-12:00"))).toBe("2026-12");
  });

  it("2026-11-30T23:30 at UTC+14 is still November in UTC", () => {
    expect(currentMonth(inZone("Etc/GMT-14", "2026-11-30T23:30:00+14:00"))).toBe("2026-11");
  });

  it("a person ahead of UTC on its last evening of the month is in the month that has not ended", () => {
    const now = inZone("Etc/GMT-14", "2026-11-30T20:00:00Z");
    expect(now.getMonth()).toBe(11); // December, locally
    expect(currentMonth(now)).toBe("2026-11");
  });
});
