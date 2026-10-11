import { describe, expect, it } from "vitest";
import { type Prepared, prepareSurface } from "./prepare";
import { BASIC_CATALOG, OWN_CATALOG_ID, surface } from "./testing";

/**
 * Catalog version 6, group 1 (code and files), through the validator: what a surface may name, which
 * files it may show, and the link of a pull request. The schemas are in catalog/v6-code-files.test.ts.
 */

type Rec = Record<string, unknown>;
const refused = (p: Prepared) => {
  if (p.kind !== "refused") throw new Error(`expected a refusal, got ${p.kind}`);
  return p;
};

const SHA = "b2".repeat(32);
const ZIP = "c3".repeat(32);
const files = [
  { sha256: SHA, preview: "text" as const, mimeType: "text/plain" },
  { sha256: ZIP, preview: null, mimeType: "application/zip" },
];

const code = (id: string, extra: Rec = {}): Rec => ({
  id,
  component: "Code",
  code: "x = 1",
  ...extra,
});
const diff = (id: string): Rec => ({ id, component: "Diff", diff: "-a\n+b" });
const term = (id: string): Rec => ({ id, component: "Terminal", command: "npm test", exitCode: 0 });
const pr = (id: string, extra: Rec = {}): Rec => ({
  id,
  component: "PullRequest",
  repo: "acme/demo",
  number: 7,
  title: "Add login",
  state: "merged",
  ...extra,
});
const fileCard = (id: string, artifact: string): Rec => ({ id, component: "FileCard", artifact });

const ours = (components: Rec[], catalogId = OWN_CATALOG_ID) =>
  surface(components, undefined, "v0.9.1", catalogId);

describe("the code and files components, in a surface of our catalog", () => {
  it("are drawn together, with their lowered names the renderer keeps", () => {
    const p = prepareSurface(
      ours([
        { id: "root", component: "Column", children: ["c", "d", "t", "p", "f"] },
        code("c", { title: "Setup" }),
        diff("d"),
        term("t"),
        pr("p", { url: "https://github.com/acme/demo/pull/7" }),
        fileCard("f", SHA),
      ]),
      { files },
    );
    expect(p.kind).toBe("surface");
    const spec = JSON.stringify(p.kind === "surface" ? p.spec : {});
    for (const name of ["Code", "Diff", "Terminal", "PullRequest", "FileCard"]) {
      expect(spec, name).toContain(`"vymalo.${name}"`);
    }
  });

  it("a FileCard names a file of this thread, of any kind, and never another's", () => {
    expect(prepareSurface(ours([fileCard("root", ZIP)]), { files }).kind).toBe("surface");
    const other = refused(prepareSurface(ours([fileCard("root", "0".repeat(64))]), { files }));
    expect(other.rule).toBe("artifact");
    expect(other.reason).toMatch(/not one of this thread's files/);
  });

  it("a FileCard with no files in the thread is refused", () => {
    expect(refused(prepareSurface(ours([fileCard("root", SHA)]))).rule).toBe("artifact");
  });

  it("a pull request's link is checked like a card's: user information is refused", () => {
    const bad = refused(
      prepareSurface(
        ours([pr("root", { url: "https://evil.example@github.com/acme/demo/pull/7" })]),
      ),
    );
    expect(bad.rule).toBe("url");
  });

  it("a pull request's link that is not http(s) is refused by the schema", () => {
    expect(refused(prepareSurface(ours([pr("root", { url: "javascript:alert(1)" })]))).rule).toBe(
      "schema",
    );
  });

  it("in the basic catalog none of them is drawn: they are not in its vocabulary", () => {
    const p = refused(prepareSurface(surface([code("root")], undefined, "v0.9.1", BASIC_CATALOG)));
    expect(p.rule).toBe("catalog");
  });
});

describe("a heavy component is drawn once: a second reference to it is refused (rule expansion)", () => {
  const root = (children: string[]): Rec => ({ id: "root", component: "Column", children });
  const IMG = "a1".repeat(32);
  const CLIP = "b2".repeat(32);
  const SOUND = "c3".repeat(32);
  const kept = [
    { sha256: IMG, preview: "image" as const, mimeType: "image/png" },
    { sha256: CLIP, preview: null, mimeType: "video/mp4" },
    { sha256: SOUND, preview: null, mimeType: "audio/wav" },
  ];
  const HEAVY: Record<string, Rec> = {
    Code: { component: "Code", code: "x = 1" },
    Diff: { component: "Diff", diff: "-a\n+b" },
    Terminal: { component: "Terminal", command: "npm test" },
    Table: { component: "Table", columns: [{ key: "a", label: "A" }], rows: [{ a: "1" }] },
    Chart: { component: "Chart", type: "bar", labels: ["x"], series: [{ name: "s", values: [1] }] },
    Gallery: { component: "Gallery", images: [{ artifact: IMG, alt: "A chart" }] },
    Video: { component: "Video", artifact: CLIP, title: "Demo" },
    Audio: { component: "Audio", artifact: SOUND, title: "Demo" },
    FileCard: { component: "FileCard", artifact: IMG },
  };

  for (const [name, shape] of Object.entries(HEAVY)) {
    it(`a ${name} referenced from two places is refused, the second time it is reached`, () => {
      const bad = refused(
        prepareSurface(
          ours([
            root(["left", "right"]),
            { id: "left", component: "Column", children: ["d"] },
            { id: "right", component: "Column", children: ["d"] },
            { id: "d", ...shape },
          ]),
          { files: kept },
        ),
      );
      expect(bad.rule, name).toBe("expansion");
      expect(bad.reason, name).toMatch(/reached more than once/);
    });
  }

  it("a text may be referenced twice: only the heavy components are drawn once", () => {
    const p = prepareSurface(
      ours([
        root(["left", "right"]),
        { id: "left", component: "Column", children: ["t"] },
        { id: "right", component: "Column", children: ["t"] },
        { id: "t", component: "Text", text: "Twice" },
      ]),
    );
    expect(p.kind, JSON.stringify(p)).toBe("surface");
  });
});
