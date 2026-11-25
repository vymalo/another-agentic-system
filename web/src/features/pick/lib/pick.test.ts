import { describe, expect, it } from "vitest";

// A listing the test just parsed must not be null; say so once instead of asserting at every use.
const must = <T>(value: T | null): T => {
  if (value === null) throw new Error("the listing did not parse");
  return value;
};

import {
  EFFORTS,
  MODEL_PICK_URI,
  MODEL_PROP,
  offersEfforts,
  offersModels,
  parseEffort,
  parseListing,
  pickProps,
  REASONING_PROP,
  REASONING_URI,
  shownPick,
} from "./pick";

const listing = {
  models: [
    { id: "deep", label: "Deep" },
    { id: "fast", label: "Fast" },
  ],
  modelDefault: "fast",
  efforts: ["low", "high"],
  effortDefault: "low",
};

describe("the pick of a message (ADR 0086, model-pick/v1 and reasoning/v1)", () => {
  it("names the two extensions and the two forwardedProps members the contract has", () => {
    expect(MODEL_PICK_URI).toBe("https://agents.vymalo.com/a2a/extensions/model-pick/v1");
    expect(REASONING_URI).toBe("https://agents.vymalo.com/a2a/extensions/reasoning/v1");
    expect(MODEL_PROP).toBe("vymalo.model");
    expect(REASONING_PROP).toBe("vymalo.reasoningEffort");
    expect(EFFORTS).toEqual(["none", "minimal", "low", "medium", "high", "xhigh", "max"]);
  });

  it("reads only a level of the seven, spelled exactly", () => {
    expect(parseEffort("low")).toBe("low");
    expect(parseEffort("max")).toBe("max");
    expect(parseEffort("LOW")).toBeUndefined();
    expect(parseEffort("")).toBeUndefined();
    expect(parseEffort(1)).toBeUndefined();
  });

  it("reads the listing the agent list carries, in the contract's order", () => {
    expect(parseListing(listing)).toEqual({
      models: [
        { id: "deep", label: "Deep" },
        { id: "fast", label: "Fast" },
      ],
      modelDefault: "fast",
      efforts: ["low", "high"],
      effortDefault: "low",
    });
    // Levels come back lowest first whatever order the card sent.
    expect(parseListing({ efforts: ["high", "low"] })?.efforts).toEqual(["low", "high"]);
  });

  it("draws nothing when the card lists nothing", () => {
    expect(parseListing(undefined)).toBeNull();
    expect(parseListing(null)).toBeNull();
    expect(parseListing({})).toEqual({ models: [], efforts: [] });
    const empty = must(parseListing({ models: [], efforts: [] }));
    expect(offersModels(empty)).toBe(false);
    expect(offersEfforts(empty)).toBe(false);
  });

  it("fails closed on a malformed listing", () => {
    for (const bad of [
      "x",
      [],
      { models: "deep" },
      { models: [{ id: "", label: "Deep" }] },
      { models: [{ id: "deep" }] },
      {
        models: [
          { id: "deep", label: "Deep" },
          { id: "deep", label: "Other" },
        ],
      },
      { models: [{ id: "deep", label: "Deep" }], modelDefault: "fast" },
      { efforts: "low" },
      { efforts: ["loud"] },
      { efforts: ["low", "low"] },
      { efforts: ["low"], effortDefault: "high" },
      { efforts: ["low"], effortDefault: "loud" },
    ]) {
      expect(parseListing(bad), JSON.stringify(bad)).toBeNull();
    }
  });

  it("shows the person's pick when the card still lists it, and the agent's default otherwise", () => {
    expect(shownPick(must(parseListing(listing)), { model: "deep", effort: "high" })).toEqual({
      model: { id: "deep", label: "Deep", picked: true },
      effort: { level: "high", picked: true },
    });
    // Nothing picked: the agent's own defaults apply, marked as not picked (nothing is sent).
    expect(shownPick(must(parseListing(listing)), {})).toEqual({
      model: { id: "fast", label: "Fast", picked: false },
      effort: { level: "low", picked: false },
    });
    // A choice the card no longer lists falls back to the default rather than drawing a stale chip.
    expect(shownPick(must(parseListing(listing)), { model: "gone", effort: "max" })).toEqual({
      model: { id: "fast", label: "Fast", picked: false },
      effort: { level: "low", picked: false },
    });
  });

  it("draws the agent-default entry for a listed axis the card names no default for", () => {
    // a card that lists choices and no default still has a chip (ADR 0086: the listing gates it);
    // the entry says "Agent default" and sends nothing, which is what no pick means
    const modelsOnly = must(parseListing({ models: [{ id: "deep", label: "Deep" }] }));
    expect(shownPick(modelsOnly, {})).toEqual({ model: { picked: false } });
    expect(shownPick(modelsOnly, { model: "deep" })).toEqual({
      model: { id: "deep", label: "Deep", picked: true },
    });
    const effortsOnly = must(parseListing({ efforts: ["medium"] }));
    expect(shownPick(effortsOnly, {})).toEqual({ effort: { picked: false } });
  });

  it("draws no chip on an axis the card does not list", () => {
    const effortsOnly = must(parseListing({ efforts: ["medium"] }));
    expect(shownPick(effortsOnly, { effort: "medium" })).toEqual({
      effort: { level: "medium", picked: true },
    });
    // no model axis at all: no chip to draw there
    expect(shownPick(effortsOnly, {})).toEqual({ effort: { picked: false } });
    expect(shownPick(must(parseListing({})), {})).toEqual({});
  });

  it("sends nothing the person did not pick", () => {
    expect(pickProps({})).toEqual({});
    expect(pickProps({ model: "deep" })).toEqual({ "vymalo.model": "deep" });
    expect(pickProps({ effort: "high" })).toEqual({ "vymalo.reasoningEffort": "high" });
    expect(pickProps({ model: "deep", effort: "high" })).toEqual({
      "vymalo.model": "deep",
      "vymalo.reasoningEffort": "high",
    });
    // An empty alias is no pick at all.
    expect(pickProps({ model: "" })).toEqual({});
  });
});
