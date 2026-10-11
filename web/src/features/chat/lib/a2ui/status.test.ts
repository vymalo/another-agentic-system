import { describe, expect, it } from "vitest";
import { progressOf, readEvents, readItems, STATE_WORDS } from "./status";

describe("the words of a state", () => {
  it("says each state in words, so that no state is told by its colour alone", () => {
    expect(STATE_WORDS).toEqual({
      done: "Done",
      doing: "In progress",
      todo: "To do",
      failed: "Failed",
    });
  });
});

describe("progressOf: how many of a checklist are done", () => {
  it("counts the done items, out of all of them, with the share as a percentage", () => {
    expect(
      progressOf([{ state: "done" }, { state: "doing" }, { state: "done" }, { state: "failed" }]),
    ).toEqual({ done: 2, total: 4, percent: 50 });
  });

  it("a failed or a doing item is not done", () => {
    expect(progressOf([{ state: "failed" }, { state: "doing" }, { state: "todo" }])).toEqual({
      done: 0,
      total: 3,
      percent: 0,
    });
  });

  it("is zero of zero, not a division by zero, with no items", () => {
    expect(progressOf([])).toEqual({ done: 0, total: 0, percent: 0 });
  });

  it("rounds the percentage to a whole number", () => {
    expect(progressOf([{ state: "done" }, { state: "todo" }, { state: "todo" }]).percent).toBe(33);
  });
});

describe("readItems and readEvents: what a component says, read defensively", () => {
  it("reads a checklist item's label and state, and an unknown state as to do", () => {
    expect(
      readItems([{ label: "a", state: "done" }, { label: "b", state: "later" }, { label: 1 }]),
    ).toEqual([
      { label: "a", state: "done" },
      { label: "b", state: "todo" },
    ]);
  });

  it("reads a timeline event's time, title and description, and a missing state as done", () => {
    expect(readEvents([{ time: "09:00", title: "Start", description: "Begun." }])).toEqual([
      { time: "09:00", title: "Start", description: "Begun.", state: "done" },
    ]);
  });

  it("reads nothing from something that is not a list", () => {
    expect(readItems("not a list")).toEqual([]);
    expect(readEvents(undefined)).toEqual([]);
  });
});
