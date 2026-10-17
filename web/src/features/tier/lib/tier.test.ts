import { describe, expect, it } from "vitest";
import {
  ACCESS_TIER_URI,
  defaultFor,
  offeredTiers,
  parseOffer,
  parseTier,
  shownTier,
  TIER_INFO,
  TIER_KEY,
  TIER_PROP,
  TIERS,
} from "./tier";

describe("the access tier words (ADR 0061)", () => {
  it("are the four of the contract, lowest first, under the keys and the URI it names", () => {
    expect(TIERS).toEqual(["plan", "edits", "auto", "bypass"]);
    expect(ACCESS_TIER_URI).toBe("https://agents.vymalo.com/a2a/extensions/access-tier/v1");
    expect(TIER_PROP).toBe("vymalo.tier");
    expect(TIER_KEY).toBe("vymalo.tier");
    expect(Object.keys(TIER_INFO)).toEqual([...TIERS]);
  });

  it("each has a name and the one line of what the agent may do under it", () => {
    expect(TIER_INFO.plan.description).toBe("Read and propose");
    expect(TIER_INFO.edits.description).toBe("Change files, open PRs");
    expect(TIER_INFO.auto.description).toBe("Merge and create too");
    expect(TIER_INFO.bypass.description).toBe("Anything, never asks");
  });

  it("a word is one of the four spelled exactly, and anything else is none", () => {
    for (const tier of TIERS) expect(parseTier(tier)).toBe(tier);
    for (const bad of ["Plan", "yolo", "", " plan", "plan ", 1, null, undefined, ["plan"], {}]) {
      expect(parseTier(bad), JSON.stringify(bad)).toBeUndefined();
    }
  });
});

describe("what a card lists (parseOffer)", () => {
  it("reads the tiers lowest first whatever order the card has them in, and the default", () => {
    expect(parseOffer({ tiers: ["bypass", "plan", "edits"], default: "edits" })).toEqual({
      tiers: ["plan", "edits", "bypass"],
      default: "edits",
    });
  });

  it("reads the person's cap on this agent beside them, when the document has one", () => {
    expect(
      parseOffer({ tiers: ["plan", "edits", "auto"], default: "auto", maxTier: "edits" }),
    ).toEqual({ tiers: ["plan", "edits", "auto"], default: "auto", max: "edits" });
    // no cap on this agent: no member, and no key in the offer
    expect(parseOffer({ tiers: ["plan"], default: "plan" })).toEqual({
      tiers: ["plan"],
      default: "plan",
    });
    // a cap that is not one of the four is no offer: fail closed, no chip
    for (const maxTier of ["yolo", "Plan", "", 1, null, ["plan"]]) {
      expect(
        parseOffer({ tiers: ["plan"], default: "plan", maxTier }),
        JSON.stringify(maxTier),
      ).toBeNull();
    }
  });

  it("is nothing for anything the contract does not allow, so no chip is drawn", () => {
    for (const bad of [
      undefined,
      null,
      "plan",
      ["plan"],
      {},
      { tiers: [], default: "plan" },
      { tiers: "plan", default: "plan" },
      { tiers: ["plan", "plan"], default: "plan" },
      { tiers: ["plan", "all"], default: "plan" },
      { tiers: ["Plan"], default: "Plan" },
      { tiers: ["plan"] },
      { tiers: ["plan"], default: "auto" },
      { tiers: ["plan"], default: 1 },
    ]) {
      expect(parseOffer(bad), JSON.stringify(bad)).toBeNull();
    }
  });
});

describe("what the person is offered (offeredTiers)", () => {
  const offer = { tiers: [...TIERS], default: "auto" as const };

  it("is every tier the agent honours when the roles have no cap", () => {
    expect(offeredTiers(offer)).toEqual([...TIERS]);
  });

  it("stops at the highest the person's roles allow", () => {
    expect(offeredTiers({ ...offer, max: "auto" })).toEqual(["plan", "edits", "auto"]);
    expect(offeredTiers({ ...offer, max: "plan" })).toEqual(["plan"]);
    expect(offeredTiers({ ...offer, max: "bypass" })).toEqual([...TIERS]);
  });

  it("keeps only what the agent honours under the cap", () => {
    expect(offeredTiers({ tiers: ["plan", "bypass"], default: "plan", max: "auto" })).toEqual([
      "plan",
    ]);
  });

  it("can be nothing: a cap below everything the agent honours", () => {
    expect(offeredTiers({ tiers: ["auto", "bypass"], default: "auto", max: "plan" })).toEqual([]);
  });
});

describe("what runs without a pick (defaultFor)", () => {
  const offer = { tiers: [...TIERS], default: "auto" as const };

  it("is the agent's default when the person's roles hold nothing back", () => {
    expect(defaultFor(offer, [...TIERS])).toBe("auto");
    expect(defaultFor(offer, offeredTiers({ ...offer, max: "bypass" }))).toBe("auto");
    expect(defaultFor(offer, offeredTiers({ ...offer, max: "auto" }))).toBe("auto");
  });

  it("is the highest tier below the default that the person's role allows", () => {
    expect(defaultFor(offer, offeredTiers({ ...offer, max: "edits" }))).toBe("edits");
    expect(defaultFor(offer, offeredTiers({ ...offer, max: "plan" }))).toBe("plan");
  });

  it("skips a tier the card does not list", () => {
    const skipping = { tiers: ["plan", "auto"] as const, default: "auto" as const };
    expect(defaultFor(skipping, offeredTiers({ ...skipping, max: "edits" }))).toBe("plan");
  });

  it("is never above the default, however much the role allows", () => {
    const low = { tiers: ["plan", "edits", "auto"] as const, default: "plan" as const };
    expect(defaultFor(low, offeredTiers({ ...low, max: "auto" }))).toBe("plan");
  });

  it("is nothing when the agent offers no tier the person may use", () => {
    const high = { tiers: ["auto", "bypass"] as const, default: "auto" as const };
    expect(defaultFor(high, offeredTiers({ ...high, max: "edits" }))).toBeUndefined();
  });
});

describe("what the chip shows (shownTier)", () => {
  const offer = { tiers: [...TIERS], default: "auto" as const };

  it("is the agent's default until the person picks, and that sends nothing", () => {
    expect(shownTier(offer, [...TIERS], undefined)).toEqual({
      tier: "auto",
      picked: false,
      capped: false,
    });
  });

  it("is the pick while it is offered", () => {
    expect(shownTier(offer, [...TIERS], "plan")).toEqual({
      tier: "plan",
      picked: true,
      capped: false,
    });
  });

  it("is the tier that runs without a pick for a pick that is not offered any more", () => {
    expect(shownTier(offer, ["plan", "edits"], "bypass")).toEqual({
      tier: "edits",
      picked: false,
      capped: true,
    });
  });

  it("is the highest the role allows, not the agent's default, for a capped person who picked nothing", () => {
    // the cap is `edits` and the agent works under `auto`: the orchestrator runs the message at `edits`
    expect(shownTier(offer, offeredTiers({ ...offer, max: "edits" }), undefined)).toEqual({
      tier: "edits",
      picked: false,
      capped: true,
    });
  });

  it("is nothing when the person may use no tier of the agent and picked none", () => {
    const high = { tiers: ["auto", "bypass"] as const, default: "auto" as const };
    expect(shownTier(high, offeredTiers({ ...high, max: "edits" }), undefined)).toBeNull();
  });
});
