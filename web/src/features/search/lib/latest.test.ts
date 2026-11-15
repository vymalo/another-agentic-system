import { describe, expect, it } from "vitest";
import { Latest } from "./latest";

describe("Latest", () => {
  it("counts only the newest request", () => {
    const latest = new Latest();
    const first = latest.next();
    expect(latest.is(first)).toBe(true);
    const second = latest.next();
    expect(latest.is(first)).toBe(false);
    expect(latest.is(second)).toBe(true);
  });

  it("drops what was in flight when the box is cleared", () => {
    const latest = new Latest();
    const n = latest.next();
    latest.cancel();
    expect(latest.is(n)).toBe(false);
  });

  it("drops a slow answer for an earlier text", async () => {
    const latest = new Latest();
    const drawn: string[] = [];
    const ask = async (text: string, ms: number) => {
      const n = latest.next();
      await new Promise((resolve) => setTimeout(resolve, ms));
      if (latest.is(n)) drawn.push(text);
    };
    await Promise.all([ask("lo", 30), ask("login", 5)]);
    expect(drawn).toEqual(["login"]);
  });
});
