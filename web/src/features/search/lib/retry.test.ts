import { describe, expect, it } from "vitest";
import { retryAfterSeconds } from "./retry";

const NOW = Date.parse("2026-11-15T10:00:00Z");

describe("retryAfterSeconds", () => {
  it("reads seconds, at least one and at most thirty", () => {
    expect(retryAfterSeconds("2", NOW)).toBe(2);
    expect(retryAfterSeconds("0", NOW)).toBe(1);
    expect(retryAfterSeconds("3600", NOW)).toBe(30);
  });
  it("reads an HTTP date", () => {
    expect(retryAfterSeconds("Sun, 15 Nov 2026 10:00:05 GMT", NOW)).toBe(5);
    expect(retryAfterSeconds("Sun, 15 Nov 2026 09:00:00 GMT", NOW)).toBe(1);
  });
  it("is one second for nothing or for nonsense", () => {
    expect(retryAfterSeconds(null, NOW)).toBe(1);
    expect(retryAfterSeconds("soon", NOW)).toBe(1);
  });
});
