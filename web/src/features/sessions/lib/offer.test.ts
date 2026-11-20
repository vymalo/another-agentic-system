import { describe, expect, it } from "vitest";
import { CHILD_SESSIONS_URI, parseChildSessionsOffer } from "./offer";

/*
 * `child-sessions/v1` read from the capabilities document (ADR 0072, docs/api/child-sessions-v1.md
 * section 1): the same rules as the Rust reader (`ChildSessionsOffer::from_params`) and the contract's
 * bound on `maxOpen`, case by case. A badly formed entry is no offer at all, never a shorter one.
 */

const good = {
  agents: [
    { name: "planner", description: "Breaks a request into steps." },
    { name: "researcher", description: "Looks things up on the web." },
  ],
  maxOpen: 4,
  later: "ignored",
};

describe("CHILD_SESSIONS_URI", () => {
  it("is the contract's URI", () => {
    expect(CHILD_SESSIONS_URI).toBe("https://agents.vymalo.com/a2a/extensions/child-sessions/v1");
  });
});

describe("parseChildSessionsOffer", () => {
  it("reads the helpers in the card's order and the limit, ignoring unknown members", () => {
    expect(parseChildSessionsOffer(good)).toEqual({
      agents: [
        { name: "planner", description: "Breaks a request into steps." },
        { name: "researcher", description: "Looks things up on the web." },
      ],
      maxOpen: 4,
    });
  });

  it("keeps a description that is empty (it is a string, and the card's words are shown as they are)", () => {
    expect(
      parseChildSessionsOffer({ agents: [{ name: "a", description: "" }], maxOpen: 1 }),
    ).toEqual({
      agents: [{ name: "a", description: "" }],
      maxOpen: 1,
    });
  });

  it.each([
    ["null", null],
    ["a string", "planner"],
    ["an array", [good]],
    ["no agents", { maxOpen: 4 }],
    ["an empty list", { agents: [], maxOpen: 4 }],
    ["agents that is not an array", { agents: { name: "a", description: "d" }, maxOpen: 4 }],
    ["an entry that is not an object", { agents: ["planner"], maxOpen: 4 }],
    ["a name that is not a string", { agents: [{ name: 3, description: "d" }], maxOpen: 4 }],
    ["a missing name", { agents: [{ description: "d" }], maxOpen: 4 }],
    ["an empty name", { agents: [{ name: "", description: "d" }], maxOpen: 4 }],
    ["a blank name", { agents: [{ name: " \t", description: "d" }], maxOpen: 4 }],
    ["a missing description", { agents: [{ name: "a" }], maxOpen: 4 }],
    ["a description that is not a string", { agents: [{ name: "a", description: 1 }], maxOpen: 4 }],
    [
      "a name twice",
      {
        agents: [
          { name: "a", description: "x" },
          { name: "a", description: "y" },
        ],
        maxOpen: 4,
      },
    ],
    ["no maxOpen", { agents: good.agents }],
  ])("is no offer for %s", (_label, params) => {
    expect(parseChildSessionsOffer(params)).toBeNull();
  });

  it("counts a name that differs only by case as another name", () => {
    const offer = parseChildSessionsOffer({
      agents: [
        { name: "Planner", description: "x" },
        { name: "planner", description: "y" },
      ],
      maxOpen: 2,
    });
    expect(offer?.agents.map((a) => a.name)).toEqual(["Planner", "planner"]);
  });

  describe("maxOpen", () => {
    const withMax = (maxOpen: unknown) => parseChildSessionsOffer({ agents: good.agents, maxOpen });

    it.each([1, 4, 16, 255, 256])("takes the whole number %s", (n) => {
      expect(withMax(n)?.maxOpen).toBe(n);
    });

    it("counts a whole double, as an SDK hands one back: 16.0 is 16", () => {
      expect(withMax(16.0)?.maxOpen).toBe(16);
      expect(withMax(1)?.maxOpen).toBe(1);
    });

    it.each([
      ["zero", 0],
      ["negative", -1],
      ["fractional", 4.5],
      ["above the most a thread notes", 257],
      ["far above it", 4_294_967_296],
      ["not finite", Number.POSITIVE_INFINITY],
      ["not a number", Number.NaN],
      ["a string", "4"],
      ["a boolean", true],
      ["null", null],
      ["absent", undefined],
    ])("is no offer when it is %s", (_label, value) => {
      expect(withMax(value)).toBeNull();
    });
  });
});
