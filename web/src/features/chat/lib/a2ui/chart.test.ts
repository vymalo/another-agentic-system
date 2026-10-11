import { describe, expect, it } from "vitest";
import { arcPath, niceTicks, pieSlices, readChart } from "./chart";

describe("readChart", () => {
  const chart = (over: Record<string, unknown> = {}) => ({
    type: "bar",
    labels: ["Mon", "Tue"],
    series: [{ name: "Runs", values: [3, 5] }],
    ...over,
  });

  it("reads a chart of each type, with its title, caption and unit", () => {
    expect(readChart(chart({ title: "Runs", caption: "Per day", unit: "runs" }))).toEqual({
      type: "bar",
      title: "Runs",
      caption: "Per day",
      unit: "runs",
      labels: ["Mon", "Tue"],
      series: [{ name: "Runs", values: [3, 5] }],
    });
    expect(readChart(chart({ type: "line" }))?.type).toBe("line");
    expect(readChart(chart({ type: "pie", series: [{ name: "s", values: [1, 1] }] }))?.type).toBe(
      "pie",
    );
  });

  it("refuses a series whose values are not as many as the labels", () => {
    expect(readChart(chart({ series: [{ name: "s", values: [1] }] }))).toBeUndefined();
  });

  it("refuses a value that is not a finite number, and a type that is not one of the three", () => {
    expect(readChart(chart({ series: [{ name: "s", values: [1, "2"] }] }))).toBeUndefined();
    expect(readChart(chart({ type: "area" }))).toBeUndefined();
  });
});

describe("niceTicks", () => {
  it("steps in 1, 2, 2.5, 5 or 10 times a power of ten, from at most the minimum to at least the maximum", () => {
    expect(niceTicks(0, 100)).toEqual([0, 25, 50, 75, 100]);
    expect(niceTicks(0, 10)).toEqual([0, 2.5, 5, 7.5, 10]);
    expect(niceTicks(-5, 12)).toEqual([-5, 0, 5, 10, 15]);
  });

  it("always includes zero when the range holds it, and covers both ends", () => {
    const ticks = niceTicks(3, 97);
    expect(ticks[0]).toBeLessThanOrEqual(0);
    expect(ticks.at(-1)).toBeGreaterThanOrEqual(97);
  });

  it("is one tick for a range of nothing, not a loop", () => {
    expect(niceTicks(0, 0)).toEqual([0]);
  });

  it("ends on a range too small to step or too large to measure, with the two ends, never a loop of NaN", () => {
    expect(niceTicks(0, 5e-324)).toEqual([0, 5e-324]);
    expect(niceTicks(0, Number.POSITIVE_INFINITY)).toEqual([0, Number.POSITIVE_INFINITY]);
    expect(niceTicks(-1e308, 1e308).length).toBeLessThanOrEqual(50);
  });
});

describe("pieSlices", () => {
  it("cuts the turn clockwise from twelve o'clock, in proportion to each value", () => {
    const slices = pieSlices([1, 3]);
    expect(slices.map((s) => s.fraction)).toEqual([0.25, 0.75]);
    expect(slices[0]).toEqual({ start: 0, end: 90, fraction: 0.25 });
    expect(slices[1]?.end).toBeCloseTo(360);
  });

  it("draws nothing for a negative value, and nothing for a total of zero", () => {
    expect(pieSlices([-2, 4]).map((s) => s.fraction)).toEqual([0, 1]);
    expect(pieSlices([0, 0])).toEqual([]);
  });
});

describe("arcPath", () => {
  it("is a wedge from the centre, for a slice, and two half arcs for a whole circle", () => {
    expect(arcPath(50, 50, 40, 0, 90)).toMatch(
      /^M 50 50 L 50\.00 10\.00 A 40 40 0 0 1 90\.00 50\.00 Z$/,
    );
    expect(arcPath(50, 50, 40, 0, 360)).toMatch(/^M .* A 40 40 0 1 1 .* A 40 40 0 1 1 .* Z$/);
    expect(arcPath(50, 50, 40, 0, 270)).toMatch(/ 0 1 1 /);
  });
});
