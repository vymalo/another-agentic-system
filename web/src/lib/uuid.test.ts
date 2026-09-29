import { describe, expect, it } from "vitest";
import { uuidv7 } from "./uuid";

describe("uuidv7", () => {
  it("is a version 7, variant 10 UUID that carries the time", () => {
    const id = uuidv7(0x0190_1234_5678);
    expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(id.replaceAll("-", "").slice(0, 12)).toBe("019012345678");
  });

  it("sorts by the time it was made, as the thread list needs", () => {
    const t = Date.UTC(2026, 8, 29);
    const ids = Array.from({ length: 50 }, (_, i) => uuidv7(t + i));
    expect([...ids].sort()).toEqual(ids);
    expect(new Set(uuidv7(t)).size).toBeGreaterThan(1); // random tail: not a constant
  });

  it("uses the random source it is given for everything but the time and the markers", () => {
    const id = uuidv7(0, (b) => b.fill(0xff));
    expect(id).toBe("00000000-0000-7fff-bfff-ffffffffffff");
  });
});
