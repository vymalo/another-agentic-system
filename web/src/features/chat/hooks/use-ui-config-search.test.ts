import { describe, expect, it } from "vitest";
import { searchOf } from "./use-ui-config";

describe("searchOf: `search` of GET /api/config is the capability", () => {
  it("is nothing for no search, a partial one, a zero, a fraction or a string", () => {
    expect(searchOf(undefined)).toBeUndefined();
    expect(searchOf(null)).toBeUndefined();
    expect(searchOf({ maxTerms: 8 })).toBeUndefined();
    expect(searchOf({ maxQueryLength: 0, maxTerms: 8 })).toBeUndefined();
    expect(searchOf({ maxQueryLength: 1.5, maxTerms: 8 })).toBeUndefined();
    expect(searchOf({ maxQueryLength: "200", maxTerms: 8 })).toBeUndefined();
  });

  it("is its numbers when both are whole and positive", () => {
    expect(searchOf({ maxQueryLength: 200, maxTerms: 8 })).toEqual({
      maxQueryLength: 200,
      maxTerms: 8,
    });
  });
});
