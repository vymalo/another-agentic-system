import { describe, expect, it } from "vitest";
import { CI_CONCLUSIONS, conclusionLabel, isKnownConclusion, providerLabel } from "./ci";

describe("a CI conclusion in words", () => {
  it("every conclusion of the schema has its own label", () => {
    expect(CI_CONCLUSIONS.map(conclusionLabel)).toEqual([
      "Success",
      "Neutral",
      "Skipped",
      "Failure",
      "Cancelled",
      "Timed out",
      "Action required",
      "Stale",
      "Startup failure",
    ]);
    expect(new Set(CI_CONCLUSIONS.map(conclusionLabel)).size).toBe(CI_CONCLUSIONS.length);
    expect(CI_CONCLUSIONS.every(isKnownConclusion)).toBe(true);
  });

  it("one it does not know is shown by its own name, cut short (it is untrusted)", () => {
    expect(isKnownConclusion("exploded")).toBe(false);
    expect(conclusionLabel("partial_success")).toBe("Partial success");
    expect(conclusionLabel("x".repeat(100)).length).toBeLessThanOrEqual(41);
  });

  it("names the provider, and shortens one it does not know", () => {
    expect(providerLabel("github")).toBe("GitHub");
    expect(providerLabel("generic")).toBe("Generic webhook");
    expect(providerLabel("gitlab")).toBe("gitlab");
    expect(providerLabel("g".repeat(100)).length).toBeLessThanOrEqual(41);
  });
});
