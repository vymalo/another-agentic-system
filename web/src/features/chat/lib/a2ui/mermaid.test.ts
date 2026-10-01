// @vitest-environment jsdom
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  MAX_MERMAID_CODE,
  mermaidConfig,
  readMermaid,
  reasonOf,
  SECURE_KEYS,
  svgImage,
  TOKEN_NAMES,
  tokensFrom,
} from "./mermaid";

describe("readMermaid", () => {
  it("reads the source, the title and the caption", () => {
    expect(readMermaid({ code: "graph TD; A-->B", title: "T", caption: "C" })).toEqual({
      code: "graph TD; A-->B",
      title: "T",
      caption: "C",
    });
    expect(readMermaid({ code: "graph TD; A-->B", title: "", caption: "" })).toEqual({
      code: "graph TD; A-->B",
    });
  });

  it("is not a Mermaid without a source that says something", () => {
    expect(readMermaid({})).toBeUndefined();
    expect(readMermaid({ code: 5 })).toBeUndefined();
    expect(readMermaid({ code: "" })).toBeUndefined();
    expect(readMermaid({ code: "  \n " })).toBeUndefined();
  });
});

describe("the colours", () => {
  const css = (values: Record<string, string>) => (name: string) => values[name] ?? "";

  it("come from the page's custom properties, trimmed", () => {
    const t = tokensFrom(css({ "--card": " #101010 ", "--brand": "rgb(1, 2, 3)" }), "light");
    expect(t.card).toBe("#101010");
    expect(t.brand).toBe("rgb(1, 2, 3)");
  });

  it("fall back, for a token the page does not define or that mermaid could not read, to the palette of the scheme", () => {
    const light = tokensFrom(css({}), "light");
    const dark = tokensFrom(css({ "--card": "oklch(0.2 0 0)", "--muted": "var(--x)" }), "dark");
    for (const name of TOKEN_NAMES) {
      expect(light[name], name).toMatch(/^#[0-9a-f]{6}$/);
      expect(dark[name], name).toMatch(/^#[0-9a-f]{6}$/);
    }
    expect(light.background).toBe("#ffffff");
    expect(dark.background).toBe("#141614");
    expect(dark.card).toBe("#1a1d1a"); // oklch() would throw inside mermaid
  });
});

describe("the configuration given to mermaid", () => {
  const tokens = tokensFrom(() => "", "light");

  it("is strict, with no HTML labels, no start on load, no error drawing and the schema's size", () => {
    const c = mermaidConfig(tokens, "light");
    expect(c).toMatchObject({
      startOnLoad: false,
      securityLevel: "strict",
      htmlLabels: false,
      suppressErrorRendering: true,
      maxTextSize: MAX_MERMAID_CODE,
      theme: "base",
      darkMode: false,
      look: "classic",
      layout: "dagre",
    });
    expect(MAX_MERMAID_CODE).toBe(20_000);
  });

  it("keeps a graph's own directive away from everything that matters", () => {
    const c = mermaidConfig(tokens, "light") as { secure: string[] };
    for (const key of [
      "securityLevel",
      "htmlLabels",
      "theme",
      "themeVariables",
      "themeCSS",
      "look",
      "layout",
      "dompurifyConfig",
      "maxTextSize",
    ]) {
      expect(c.secure, key).toContain(key);
    }
    expect(c.secure).toBe(SECURE_KEYS);
  });

  it("is drawn in the colours of the page, in dark mode as in light", () => {
    const dark = tokensFrom(() => "", "dark");
    const d = mermaidConfig(dark, "dark") as {
      darkMode: boolean;
      themeVariables: Record<string, string>;
    };
    expect(d.darkMode).toBe(true);
    expect(d.themeVariables).toMatchObject({
      background: dark.card,
      textColor: dark.foreground,
      primaryColor: dark.muted,
      primaryBorderColor: dark.input,
      lineColor: dark["muted-foreground"],
    });
    const l = mermaidConfig(tokens, "light") as { themeVariables: Record<string, string> };
    expect(l.themeVariables.background).toBe(tokens.card);
    expect(l.themeVariables.background).not.toBe(d.themeVariables.background);
    // every colour is one mermaid reads (khroma: hex, rgb, hsl)
    for (const [key, value] of Object.entries(d.themeVariables)) {
      if (/color|bkg|background/i.test(key)) expect(value, key).toMatch(/^#[0-9a-f]{6}$/);
    }
  });
});

describe("svgImage", () => {
  const SVG = (root: string, body = "<g/>") => `<svg id="mmd-1" ${root}>${body}</svg>`;
  /** What mermaid returns for a root: 100% wide, with a `max-width` style. */
  const mermaidRoot =
    'width="100%" xmlns="http://www.w3.org/2000/svg" class="flowchart" style="max-width: 345.5px; background-color: transparent;" viewBox="0 0 345.5 120.25" role="graphics-document document"';
  const xmlOf = (image: { src: string } | undefined) =>
    decodeURIComponent(image?.src.split(",").slice(1).join(",") ?? "");

  it("is an image source with the natural size of the viewBox, the root's width and style replaced", () => {
    const image = svgImage(SVG(mermaidRoot));
    expect(image).toMatchObject({ width: 346, height: 121 });
    expect(image?.src.startsWith("data:image/svg+xml;charset=utf-8,")).toBe(true);
    const xml = xmlOf(image);
    expect(xml).toContain('xmlns="http://www.w3.org/2000/svg"');
    expect(xml).toContain('width="346"');
    expect(xml).toContain('height="121"');
    expect(xml).not.toContain("100%");
    expect(xml).not.toContain("max-width");
    expect(xml).toContain('viewBox="0 0 345.5 120.25"');
    expect(xml).toContain("<g/>");
  });

  it("keeps the case of the attributes and elements an image (parsed as XML) needs, from a string written for innerHTML in any case", () => {
    const body =
      '<defs><marker id="m" viewbox="0 0 10 10" refx="5" refy="5" markerwidth="8" markerheight="8" markerunits="userspaceonuse" orient="auto"><path d="M0 0L10 5L0 10z"/></marker><filter id="f"><fegaussianblur stddeviation="2"/></filter></defs>';
    const xml = xmlOf(svgImage(SVG(mermaidRoot, body)));
    expect(xml).not.toContain("viewbox");
    expect(xml).toContain('viewBox="0 0 345.5 120.25"');
    for (const attr of ["viewBox", "refX", "refY", "markerWidth", "markerHeight", "markerUnits"]) {
      expect(xml, attr).toContain(`${attr}=`);
    }
    expect(xml).toContain("stdDeviation=");
    expect(xml).toContain("<feGaussianBlur");
  });

  it("is well-formed XML: text and style are escaped", () => {
    const xml = xmlOf(
      svgImage(SVG(mermaidRoot, "<text>a &lt; b &amp; c</text><style>g > p {fill:red}</style>")),
    );
    const doc = new DOMParser().parseFromString(xml, "image/svg+xml");
    expect(doc.querySelector("parsererror")).toBeNull();
    expect(doc.documentElement.localName).toBe("svg");
    expect(doc.querySelector("text")?.textContent).toBe("a < b & c");
    expect(doc.querySelector("style")?.textContent).toBe("g > p {fill:red}");
  });

  it("an HTML entity that XML does not know (a non-breaking space) becomes the character, not a broken image", () => {
    // innerHTML writes U+00A0 as &nbsp;, which an image, parsed as XML, rejects
    const xml = xmlOf(svgImage(SVG(mermaidRoot, "<text>a&nbsp;b &copy; c</text>")));
    expect(xml).not.toContain("&nbsp;");
    expect(xml).not.toContain("&copy;");
    const doc = new DOMParser().parseFromString(xml, "image/svg+xml");
    expect(doc.querySelector("parsererror")).toBeNull();
    expect(doc.querySelector("text")?.textContent).toBe("a\u00a0b \u00a9 c");
  });

  it("leaves the body alone, even where it says width or style", () => {
    const body = '<rect width="10" style="fill:red"/>';
    const xml = xmlOf(svgImage(SVG(mermaidRoot, body)));
    expect(xml).toContain('<rect width="10" style="fill:red"/>');
  });

  it("is not drawn when it is not exactly one SVG element", () => {
    expect(svgImage("")).toBeUndefined();
    expect(svgImage("<div>hi</div>")).toBeUndefined();
    expect(svgImage(`<p>${SVG(mermaidRoot)}</p>`)).toBeUndefined();
    expect(svgImage(`${SVG(mermaidRoot)}${SVG(mermaidRoot)}`)).toBeUndefined();
    expect(svgImage(`${SVG(mermaidRoot)}<p>after</p>`)).toBeUndefined();
    expect(svgImage("x".repeat(2_000_001))).toBeUndefined();
  });

  it("is not drawn without a viewBox that sizes it, or with a size that is not one", () => {
    for (const box of [undefined, "0 0 0 10", "0 0 10 -5", "0 0 x y", "0 0 30000 10", "0 0 10"]) {
      const root = box === undefined ? "" : `viewBox="${box}"`;
      expect(svgImage(SVG(root)), String(box)).toBeUndefined();
    }
  });

  it("takes a viewBox whose origin is not zero", () => {
    expect(svgImage(SVG('viewBox="4 4 360 512.8"'))).toMatchObject({ width: 360, height: 513 });
  });

  it("is not drawn with a script, a frame, a foreignObject or the like in it, in any spelling", () => {
    for (const body of [
      "<script>alert(1)</script>",
      "<SCRIPT>alert(1)</SCRIPT>",
      "<script\n>alert(1)</script>",
      "<g><script/></g>",
      '<foreignObject><div xmlns="http://www.w3.org/1999/xhtml">x</div></foreignObject>',
      '<foreignobject width="1" height="1"/>',
      '<iframe src="https://x.example/"/>',
      '<object data="x"/>',
      '<embed src="x"/>',
      '<link rel="stylesheet" href="https://x.example/s.css"/>',
    ]) {
      expect(svgImage(SVG(mermaidRoot, body)), body).toBeUndefined();
    }
    // text that merely says so is text
    expect(
      svgImage(SVG(mermaidRoot, "<text>&lt;script&gt;alert(1)&lt;/script&gt;</text>")),
    ).toBeDefined();
  });

  it("is not drawn with an event handler, in any case", () => {
    for (const body of [
      '<rect onclick="alert(1)"/>',
      '<rect onerror="alert(1)"/>',
      '<rect ONLOAD="alert(1)"/>',
      '<g><circle onmouseover="alert(1)"/></g>',
    ]) {
      expect(svgImage(SVG(mermaidRoot, body)), body).toBeUndefined();
    }
    expect(svgImage(SVG(`${mermaidRoot} onload="alert(1)"`))).toBeUndefined();
  });

  it("is not drawn with CSS that loads something, and may refer to the picture's own parts", () => {
    for (const css of [
      "@import url(https://x.example/a.css);",
      "@import 'https://x.example/a.css';",
      "g{background:url(https://x.example/a.png)}",
      'g{background:url( "https://x.example/a.png" )}',
      "g{background:url(data:image/png;base64,AAAA)}",
    ]) {
      expect(svgImage(SVG(mermaidRoot, `<style>${css}</style>`)), css).toBeUndefined();
      expect(
        svgImage(SVG(mermaidRoot, `<rect style="${css.replace(/"/g, "&quot;")}"/>`)),
        css,
      ).toBeUndefined();
    }
    expect(
      svgImage(
        SVG(
          mermaidRoot,
          "<style>g{fill:url(#grad)} h{fill:url('#grad')} i{fill:url( \"#g\" )}</style>",
        ),
      ),
    ).toBeDefined();
  });

  it("drops a link that leads out of the picture and keeps one that points into it", () => {
    const body =
      '<a href="https://example.com/x"><rect/></a><a xlink:href="https://example.com/y"><rect/></a><use xlink:href="#shape"/><use href="#shape"/>';
    const xml = xmlOf(
      svgImage(SVG(`${mermaidRoot} xmlns:xlink="http://www.w3.org/1999/xlink"`, body)),
    );
    expect(xml).not.toContain("example.com");
    expect(xml).toContain('href="#shape"');
  });

  it("takes an XML declaration before the root", () => {
    const xml = `<?xml version="1.0" encoding="UTF-8"?>\n${SVG(mermaidRoot)}`;
    expect(svgImage(xml)).toMatchObject({ width: 346 });
  });
});

describe("reasonOf", () => {
  it("is mermaid's own message, the place and the line (not the list of what it expected), without control characters", () => {
    expect(
      reasonOf(
        new Error("Parse error on line 2:\n  A[Start] -->\n---^\nExpecting 'SEMI', 'NEWLINE'"),
      ),
    ).toBe("Parse error on line 2:\n  A[Start] -->\n---^");
    expect(reasonOf(new Error("a\u0000b\u001bc"))).toBe("a?b?c");
    expect(reasonOf(new Error("l1\nl2\nl3\nl4\nl5\nl6"))).toBe("l1\nl2\nl3");
    expect(reasonOf(new Error("x".repeat(1000))).length).toBe(200);
  });

  it("says something for an error that has no message", () => {
    expect(reasonOf(undefined)).toBe("mermaid could not read the graph");
    expect(reasonOf(new Error(""))).toBe("mermaid could not read the graph");
    expect(reasonOf({ weird: true })).toBe("mermaid could not read the graph");
    expect(reasonOf("plain text")).toBe("plain text");
  });
});

/**
 * mermaid is about 120 MB of diagram types: it must reach the browser only when a graph is drawn.
 * One file imports it, with `import()`; anything that imported it statically would put it in the code
 * every thread loads (`pnpm build`'s first-load size, checked in the PR, is the other half of this).
 */
describe("mermaid is loaded on demand", () => {
  const src = path.resolve(import.meta.dirname, "../../../..");
  const sources = (dir: string): string[] =>
    readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
      e.isDirectory()
        ? sources(path.join(dir, e.name))
        : /\.(ts|tsx)$/.test(e.name) && !/\.test\.tsx?$/.test(e.name)
          ? [path.join(dir, e.name)]
          : [],
    );

  it("only mermaid-render.ts names the package, and with a dynamic import", () => {
    const named = sources(src).filter((f) =>
      /(?:from|import)\s*\(?\s*["']mermaid["']/.test(readFileSync(f, "utf8")),
    );
    expect(named.map((f) => path.relative(src, f))).toEqual([
      path.join("features/chat/lib/a2ui/mermaid-render.ts"),
    ]);
    const text = readFileSync(named[0] ?? "", "utf8");
    expect(text).toContain('await import("mermaid")');
    expect(text).not.toMatch(/^import .* from ["']mermaid["']/m);
  });
});
