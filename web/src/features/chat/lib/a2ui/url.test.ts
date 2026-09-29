import { describe, expect, it } from "vitest";
import { MAX_URL } from "./limits";
import { BAD_URLS, GOOD_URLS, NOT_TEXT } from "./testing";
import { safeHttpUrl } from "./url";

describe("safeHttpUrl", () => {
  it.each(GOOD_URLS)("accepts %s and normalises it to %s", (url, href) => {
    expect(safeHttpUrl(url)).toBe(href);
  });

  it.each(BAD_URLS.map((u) => [JSON.stringify(u), u]))("refuses %s", (_label, url) => {
    expect(safeHttpUrl(url)).toBeUndefined();
  });

  it.each(NOT_TEXT.map((v) => [JSON.stringify(v) ?? "undefined", v]))(
    "refuses a value that is not text: %s",
    (_label, value) => {
      expect(safeHttpUrl(value)).toBeUndefined();
    },
  );

  it("refuses a URL over the length limit and takes one at the limit", () => {
    const prefix = "https://example.com/";
    expect(safeHttpUrl(prefix + "a".repeat(MAX_URL - prefix.length))).toBeDefined();
    expect(safeHttpUrl(prefix + "a".repeat(MAX_URL - prefix.length + 1))).toBeUndefined();
  });

  it("an accepted URL is always absolute http(s) with a host and no credentials", () => {
    for (const [url] of GOOD_URLS) {
      const parsed = new URL(safeHttpUrl(url) as string);
      expect(["http:", "https:"]).toContain(parsed.protocol);
      expect(parsed.hostname).not.toBe("");
      expect(parsed.username + parsed.password).toBe("");
    }
  });
});
