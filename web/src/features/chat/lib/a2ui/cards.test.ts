import { describe, expect, it } from "vitest";
import { firstBadUrl, hostOf, readCards } from "./cards";
import { BAD_URLS, GOOD_URLS } from "./testing";

describe("readCards", () => {
  it("reads the cards in order, with their own words", () => {
    const spec = readCards({
      title: "Options",
      layout: "grid",
      cards: [
        {
          title: "A",
          subtitle: "sub",
          body: "text",
          url: "https://www.example.com/x",
          tags: ["t"],
        },
        { title: "B" },
      ],
    });
    expect(spec).toEqual({
      title: "Options",
      layout: "grid",
      cards: [
        {
          title: "A",
          subtitle: "sub",
          body: "text",
          href: "https://www.example.com/x",
          host: "example.com",
          tags: ["t"],
        },
        { title: "B", tags: [] },
      ],
    });
  });

  it("is a list unless it says grid, and keeps no empty subtitle, body or title", () => {
    const spec = readCards({
      title: "",
      layout: "masonry",
      cards: [{ title: "A", subtitle: "", body: "" }],
    });
    expect(spec).toEqual({ layout: "list", cards: [{ title: "A", tags: [] }] });
  });

  it("is not a Cards without a list of cards that each have a title", () => {
    expect(readCards({})).toBeUndefined();
    expect(readCards({ cards: [] })).toBeUndefined();
    expect(readCards({ cards: "x" })).toBeUndefined();
    expect(readCards({ cards: [{ subtitle: "no title" }] })).toBeUndefined();
    expect(readCards({ cards: [{ title: "ok" }, null] })).toBeUndefined();
  });

  it("a link that is not an http(s) URL is not a link, whatever else the card says", () => {
    for (const url of BAD_URLS) {
      const spec = readCards({ cards: [{ title: "A", url }] });
      expect(spec?.cards[0], url).toEqual({ title: "A", tags: [] });
    }
  });

  it("a link that is one is the normalised href, with the host it goes to", () => {
    for (const [url, href] of GOOD_URLS) {
      const card = readCards({ cards: [{ title: "A", url }] })?.cards[0];
      expect(card?.href, url).toBe(href);
      expect(card?.host, url).toBe(hostOf(href));
    }
  });

  it("only text tags are read", () => {
    expect(
      readCards({ cards: [{ title: "A", tags: ["a", 1, null, "b"] }] })?.cards[0]?.tags,
    ).toEqual(["a", "b"]);
  });
});

describe("hostOf", () => {
  it("is the host without a leading www, with its port", () => {
    expect(hostOf("https://www.Example.com/a")).toBe("example.com");
    expect(hostOf("http://localhost:3000/x")).toBe("localhost:3000");
    expect(hostOf("https://docs.rs/mermaid")).toBe("docs.rs");
    // a host that merely starts with www is not stripped
    expect(hostOf("https://wwwexample.com/")).toBe("wwwexample.com");
  });
});

describe("firstBadUrl", () => {
  it("is the position of the first card whose link is not an http(s) URL", () => {
    expect(
      firstBadUrl({ cards: [{ title: "a" }, { title: "b", url: "https://ok.example/" }] }),
    ).toBeUndefined();
    expect(
      firstBadUrl({
        cards: [
          { title: "a", url: "https://ok.example/" },
          { title: "b", url: "javascript:alert(1)" },
          { title: "c", url: "data:text/html,x" },
        ],
      }),
    ).toBe(1);
    // the schema's pattern lets this through; the rule of ADR 0013 does not
    expect(firstBadUrl({ cards: [{ title: "a", url: "https://user@evil.example/" }] })).toBe(0);
    expect(firstBadUrl({ cards: [{ title: "a", url: 5 }] })).toBe(0);
    expect(firstBadUrl({})).toBeUndefined();
  });
});
