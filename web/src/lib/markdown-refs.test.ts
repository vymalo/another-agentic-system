import { describe, expect, it } from "vitest";
import { type MdNode, markdownRefs, refUrl, textOf } from "./markdown-refs";

const urls = (markdown: string) => {
  const { refs, definitions } = markdownRefs(markdown);
  return refs.map((r) => [r.kind, refUrl(r, definitions)]);
};

describe("markdownRefs", () => {
  it("lists links and images in the order they are written, references through their definitions", () => {
    expect(
      urls(
        [
          "[a](https://a.example) ![b](shots/b.png) [c][one] ![d][two]",
          "",
          "[one]: https://one.example",
          "[two]: shots/two.png",
        ].join("\n"),
      ),
    ).toEqual([
      ["link", "https://a.example"],
      ["image", "shots/b.png"],
      ["link", "https://one.example"],
      ["image", "shots/two.png"],
    ]);
  });

  it("the first definition of a label wins, and a label with none is no reference at all (the parser reads it as text)", () => {
    expect(
      urls("[x][l] [y][nope]\n\n[l]: https://first.example\n[l]: https://second.example"),
    ).toEqual([["link", "https://first.example"]]);
  });

  it("finds nothing in code, which the chat does not draw as a link or an image", () => {
    expect(urls("`![x](a.png)` and\n\n```md\n[y](https://b.example)\n![z](c.png)\n```")).toEqual(
      [],
    );
  });

  it("an image inside a link is both, and the link's text says the image's description", () => {
    const { refs } = markdownRefs("[![the chart](shots/c.png)](https://page.example)");
    expect(refs.map((r) => r.kind)).toEqual(["link", "image"]);
    expect(textOf(refs[0]?.node as MdNode)).toBe("the chart");
  });
});
