import { describe, expect, it } from "vitest";
import { contextTooLarge, resolveFields } from "./context";
import { MAX_CONTEXT_BYTES } from "./limits";

describe("resolveFields", () => {
  const values = { "/email": "ada@example.com", "/agree": true, "#x": "" };

  it("replaces the marker of an input with its value, at any depth, and leaves the rest alone", () => {
    expect(
      resolveFields(
        { who: { $field: "/email" }, deep: { list: [{ $field: "/agree" }, 1, "a"] }, fixed: 1 },
        values,
      ),
    ).toEqual({ who: "ada@example.com", deep: { list: [true, 1, "a"] }, fixed: 1 });
  });

  it("a value nobody entered is null, and an empty string is kept", () => {
    expect(resolveFields({ a: { $field: "/nobody" }, b: { $field: "#x" } }, values)).toEqual({
      a: null,
      b: "",
    });
  });

  it("a marker cannot read what is not an input's value: no prototype, no inherited key", () => {
    for (const key of ["__proto__", "constructor", "toString", "hasOwnProperty"]) {
      expect(resolveFields({ a: { $field: key } }, values)).toEqual({ a: null });
    }
  });

  it("a marker with anything else in it is data, not a marker", () => {
    expect(resolveFields({ $field: "/email", other: 1 }, values)).toEqual({
      $field: "/email",
      other: 1,
    });
    expect(resolveFields({ $field: 7 }, values)).toEqual({ $field: 7 });
  });

  it("an object nested past any sense is cut, not walked", () => {
    let deep: unknown = { $field: "/email" };
    for (let i = 0; i < 100; i++) deep = { a: deep };
    expect(() => resolveFields(deep, values)).not.toThrow();
  });

  it("keeps a hostile key as an own property", () => {
    const hostile = JSON.parse('{"__proto__": {"polluted": true}}');
    const out = resolveFields(hostile, values) as Record<string, unknown>;
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
    expect(Object.keys(out)).toEqual(["__proto__"]);
  });
});

describe("contextTooLarge", () => {
  it("is measured in bytes: 16 KiB is fine, one more is not", () => {
    const at = (bytes: number) => ({ v: "a".repeat(bytes - JSON.stringify({ v: "" }).length) });
    expect(contextTooLarge(at(MAX_CONTEXT_BYTES))).toBe(false);
    expect(contextTooLarge(at(MAX_CONTEXT_BYTES + 1))).toBe(true);
    expect(contextTooLarge({ v: "é".repeat(MAX_CONTEXT_BYTES / 2) })).toBe(true);
  });
});
