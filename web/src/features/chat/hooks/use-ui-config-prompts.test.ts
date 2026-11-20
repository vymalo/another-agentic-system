import { describe, expect, it } from "vitest";
import { promptsOf } from "./use-ui-config";

describe("promptsOf: `prompts` of GET /api/config is the capability", () => {
  it("is nothing for no library, a partial one, a zero, a fraction or a string", () => {
    expect(promptsOf(undefined)).toBeUndefined();
    expect(promptsOf(null)).toBeUndefined();
    expect(promptsOf({ limit: 100, maxVariables: 8, maxBodyBytes: 16384 })).toBeUndefined();
    expect(
      promptsOf({ limit: 0, maxVariables: 8, maxBodyBytes: 16384, maxRenderedBytes: 32768 }),
    ).toBeUndefined();
    expect(
      promptsOf({ limit: "100", maxVariables: 8, maxBodyBytes: 16384, maxRenderedBytes: 32768 }),
    ).toBeUndefined();
  });

  it("is its four numbers when all are whole and positive", () => {
    const all = { limit: 100, maxVariables: 8, maxBodyBytes: 16384, maxRenderedBytes: 32768 };
    expect(promptsOf(all)).toEqual(all);
  });
});
