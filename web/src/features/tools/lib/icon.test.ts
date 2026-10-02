import { describe, expect, it } from "vitest";
import { ICON_MAX_LENGTH, iconSrc } from "./icon";

const SVG = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4=";

describe("iconSrc", () => {
  it("accepts a base64 data: URI of an svg, a png or a webp", () => {
    expect(iconSrc(SVG)).toBe(SVG);
    expect(iconSrc("data:image/png;base64,iVBORw0KGgo=")).toBe(
      "data:image/png;base64,iVBORw0KGgo=",
    );
    expect(iconSrc("data:image/webp;base64,UklGRg==")).toBe("data:image/webp;base64,UklGRg==");
  });

  it("never accepts a URL: an http(s), protocol-relative, relative or blob icon is none", () => {
    for (const icon of [
      "https://tracker.example/icon.svg",
      "http://127.0.0.1:4010/icon.png",
      "//tracker.example/icon.svg",
      "/icons/search.svg",
      "icon.svg",
      "blob:https://app.example/1234",
      "javascript:alert(1)",
      "ftp://example.org/icon.png",
    ]) {
      expect(iconSrc(icon), icon).toBeNull();
    }
  });

  it("accepts no other kind of data: URI", () => {
    for (const icon of [
      "data:text/html;base64,PGgxPmhpPC9oMT4=",
      "data:image/gif;base64,R0lGODlhAQABAAAAACw=",
      "data:image/jpeg;base64,/9j/4AAQ",
      "data:image/svg+xml,%3Csvg%3E%3C/svg%3E",
      "data:image/svg+xml;charset=utf-8;base64,PHN2Zy8+",
      "data:image/svg+xml;utf8,<svg/>",
      "data:application/octet-stream;base64,AAAA",
      "DATA:image/png;base64,iVBORw0KGgo=",
    ]) {
      expect(iconSrc(icon), icon).toBeNull();
    }
  });

  it("refuses base64 that is not base64, spaces and a second URI behind the first", () => {
    for (const icon of [
      "data:image/png;base64,",
      "data:image/png;base64,AAA AAA",
      'data:image/png;base64,AAAA" onerror="x',
      "data:image/png;base64,AAAA\nhttps://tracker.example/",
      "data:image/png;base64,AAAA)",
      " data:image/png;base64,AAAA",
    ]) {
      expect(iconSrc(icon), JSON.stringify(icon)).toBeNull();
    }
  });

  it("refuses one over 8 KiB, as the contract does", () => {
    const body = "A".repeat(ICON_MAX_LENGTH - "data:image/png;base64,".length);
    expect(iconSrc(`data:image/png;base64,${body}`)).not.toBeNull();
    expect(iconSrc(`data:image/png;base64,${body}A`)).toBeNull();
  });

  it("is none for an icon that is absent", () => {
    expect(iconSrc(undefined)).toBeNull();
    expect(iconSrc(null)).toBeNull();
    expect(iconSrc("")).toBeNull();
  });
});
