// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { readPick, writePick } from "./pick";

afterEach(() => {
  vi.restoreAllMocks();
  window.localStorage.clear();
});

describe("the pick of a thread, kept in the browser (ADR 0061)", () => {
  it("is remembered per thread and forgotten when it is cleared", () => {
    expect(readPick("t-1")).toBeUndefined();
    writePick("t-1", "edits");
    writePick("t-2", "bypass");
    expect(readPick("t-1")).toBe("edits");
    expect(readPick("t-2")).toBe("bypass");
    writePick("t-1", undefined);
    expect(readPick("t-1")).toBeUndefined();
    expect(readPick("t-2")).toBe("bypass");
  });

  it("is nothing for a stored value that is not one of the four words", () => {
    window.localStorage.setItem("vymalo.tier:t-1", "root");
    expect(readPick("t-1")).toBeUndefined();
  });

  it("works without storage: a blocked read is none and a blocked write is not an error", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("blocked", "SecurityError");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("full", "QuotaExceededError");
    });
    vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => {
      throw new DOMException("blocked", "SecurityError");
    });
    expect(readPick("t-1")).toBeUndefined();
    expect(() => writePick("t-1", "plan")).not.toThrow();
    expect(() => writePick("t-1", undefined)).not.toThrow();
  });
});
