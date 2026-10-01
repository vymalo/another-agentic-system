// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import path from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { mermaidConfig, reasonOf, SECURE_KEYS, tokensFrom } from "./mermaid";

/**
 * The real mermaid, at the version this app pins, against the options `mermaid.ts` gives it: what a
 * graph's own `%%{init}%%` directive or front matter can and cannot change. mermaid does not draw in
 * jsdom (no layout), but it parses and applies directives, which is where its security options are
 * decided; the drawing is checked in a browser (`e2e/cards.spec.ts`, the `mermaid-hostile` script).
 *
 * *Verified 2026-10-01* for `mermaid@12.0.0` (npm, MIT): `securityLevel` is in mermaid's default
 * `secure` list, so a directive cannot change it; `htmlLabels` and `theme` are not, so this app adds
 * them (and more) to `secure`.
 */

type Mermaid = typeof import("mermaid")["default"];
let mermaid: Mermaid;

const config = () => {
  const c = mermaidConfig(
    tokensFrom(() => "", "light"),
    "light",
  );
  return c as Parameters<Mermaid["initialize"]>[0];
};

beforeAll(async () => {
  mermaid = (await import("mermaid")).default;
});

/** What mermaid holds after it read `code`, with this app's configuration. */
async function configAfter(code: string) {
  mermaid.initialize(config());
  const result = await mermaid.parse(code);
  expect(result, code).toBeTruthy();
  const c = mermaid.mermaidAPI.getConfig();
  return {
    securityLevel: c.securityLevel,
    htmlLabels: c.htmlLabels,
    flowchartHtmlLabels: c.flowchart?.htmlLabels,
    theme: c.theme,
    maxTextSize: c.maxTextSize,
    themeCSS: c.themeCSS,
    fontFamily: c.fontFamily,
    look: c.look,
    layout: c.layout,
  };
}

const CLEAN = {
  securityLevel: "strict",
  htmlLabels: false,
  flowchartHtmlLabels: undefined as boolean | undefined,
  theme: "base",
  maxTextSize: 20_000,
  themeCSS: undefined as string | undefined,
  look: "classic",
  layout: "dagre",
};

describe("mermaid at the pinned version", () => {
  it("is pinned exactly in package.json, and is the installed one", () => {
    const manifest = JSON.parse(
      readFileSync(path.resolve(import.meta.dirname, "../../../../../package.json"), "utf8"),
    ) as { dependencies: Record<string, string> };
    expect(manifest.dependencies.mermaid).toMatch(/^\d+\.\d+\.\d+$/);
    const installed = JSON.parse(
      readFileSync(
        path.resolve(import.meta.dirname, "../../../../../node_modules/mermaid/package.json"),
        "utf8",
      ),
    ) as { version: string; license: string };
    expect(installed.version).toBe(manifest.dependencies.mermaid);
    expect(installed.license).toBe("MIT");
  });

  it("our `secure` list holds every key mermaid itself keeps away from a diagram", () => {
    const own = mermaid.mermaidAPI.defaultConfig.secure ?? [];
    expect(own).toContain("securityLevel");
    for (const key of own) expect(SECURE_KEYS, key).toContain(key);
  });

  it("starts from our configuration: strict, no HTML labels", async () => {
    expect(await configAfter("graph TD; A-->B")).toMatchObject(CLEAN);
  });

  it("a directive cannot loosen the security level, turn HTML labels on, or change the theme", async () => {
    const directive =
      '%%{init: {"securityLevel": "loose", "htmlLabels": true, "flowchart": {"htmlLabels": true}, "theme": "dark", "themeCSS": ".node{fill:red}", "look": "handDrawn", "layout": "elk", "maxTextSize": 999999999}}%%\ngraph TD; A-->B';
    const c = await configAfter(directive);
    expect(c).toMatchObject(CLEAN);
    expect(c.flowchartHtmlLabels).not.toBe(true);
  });

  it("front matter cannot either", async () => {
    const frontMatter = [
      "---",
      "config:",
      "  securityLevel: loose",
      "  htmlLabels: true",
      "  theme: forest",
      "  flowchart:",
      "    htmlLabels: true",
      "---",
      "graph TD; A-->B",
    ].join("\n");
    const c = await configAfter(frontMatter);
    expect(c).toMatchObject(CLEAN);
    expect(c.flowchartHtmlLabels).not.toBe(true);
  });

  it("the other spelling of a directive (`initialize`, `config`) cannot either", async () => {
    const c = await configAfter(
      '%%{initialize: {"securityLevel": "loose", "htmlLabels": true}}%%\ngraph TD; A-->B',
    );
    expect(c).toMatchObject(CLEAN);
  });

  it("a graph that does not parse is a rejection with a message the page can show", async () => {
    mermaid.initialize(config());
    const bad = "flowchart TD\n  A[Start] -->\n  B{{ not a graph";
    const error = await mermaid.parse(bad).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(Error);
    expect(reasonOf(error)).toMatch(/\S/);
    expect(reasonOf(error).length).toBeLessThanOrEqual(300);
  });

  it("control: with mermaid's own `secure` list only, the same directive does turn HTML labels on and change the theme", async () => {
    // why `SECURE_KEYS` is longer than mermaid's list: this is what it keeps out
    const own = mermaid.mermaidAPI.defaultConfig.secure;
    mermaid.initialize({ ...config(), secure: own });
    await mermaid.parse(
      '%%{init: {"htmlLabels": true, "theme": "dark", "securityLevel": "loose"}}%%\ngraph TD; A-->B',
    );
    const c = mermaid.mermaidAPI.getConfig();
    expect(c.securityLevel).toBe("strict"); // mermaid's own list does hold this one
    expect(c.htmlLabels).toBe(true);
    expect(c.theme).toBe("dark");
  });
});
