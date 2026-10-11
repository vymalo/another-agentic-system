import { describe, expect, it } from "vitest";
import { type Prepared, prepareSurface } from "./prepare";
import { BASIC_CATALOG, OWN_CATALOG_ID, surface } from "./testing";

/**
 * Catalog version 6, group 4 (media), through the validator: a Gallery's pictures are images of this thread, a Video
 * is a video of this thread and an Audio a sound of it (rule artifact), a LinkPreview's link is checked like a card's,
 * and the names the renderer keeps. The schemas are in catalog/v6-media.test.ts.
 */

type Rec = Record<string, unknown>;
const refused = (p: Prepared) => {
  if (p.kind !== "refused") throw new Error(`expected a refusal, got ${p.kind}`);
  return p;
};
const ours = (components: Rec[], catalogId = OWN_CATALOG_ID) =>
  surface(components, undefined, "v0.9.1", catalogId);
const root = (children: string[]): Rec => ({ id: "root", component: "Column", children });

const PIC = "b2".repeat(32);
const SOUND = "c3".repeat(32);
const CLIP = "d4".repeat(32);
const TXT = "e5".repeat(32);
const files = [
  { sha256: PIC, preview: "image" as const, mimeType: "image/png" },
  { sha256: SOUND, preview: null, mimeType: "audio/wav" },
  { sha256: CLIP, preview: null, mimeType: "video/mp4" },
  { sha256: TXT, preview: "text" as const, mimeType: "text/plain" },
];

const gallery = (artifact: string): Rec => ({
  id: "g",
  component: "Gallery",
  images: [{ artifact, alt: "A chart" }],
});
const video = (artifact: string): Rec => ({ id: "v", component: "Video", artifact, title: "Demo" });
const audio = (artifact: string): Rec => ({ id: "a", component: "Audio", artifact, title: "Tone" });
const link = (extra: Rec = {}): Rec => ({
  id: "l",
  component: "LinkPreview",
  url: "https://www.postgresql.org/docs/current/",
  title: "PostgreSQL documentation",
  ...extra,
});

describe("Gallery, Video, Audio and LinkPreview, in a surface of our catalog", () => {
  it("are drawn, lowered to the names the renderer keeps", () => {
    const p = prepareSurface(
      ours([root(["g", "v", "a", "l"]), gallery(PIC), video(CLIP), audio(SOUND), link()]),
      { files },
    );
    expect(p.kind).toBe("surface");
    const spec = JSON.stringify(p.kind === "surface" ? p.spec : {});
    for (const name of ["Gallery", "Video", "Audio", "LinkPreview"]) {
      expect(spec, name).toContain(`"vymalo.${name}"`);
    }
  });

  it("a Gallery's pictures are images of this thread: another file is refused (rule artifact)", () => {
    const notAnImage = refused(prepareSurface(ours([root(["g"]), gallery(TXT)]), { files }));
    expect(notAnImage.rule).toBe("artifact");
    expect(notAnImage.reason).toMatch(/picture 1 names a file that is not an image/);
    const unknown = refused(
      prepareSurface(ours([root(["g"]), gallery("0".repeat(64))]), { files }),
    );
    expect(unknown.rule).toBe("artifact");
    expect(unknown.reason).toMatch(/not one of this thread's files/);
  });

  it("a Video names a video of this thread, and an Audio a sound (rule artifact)", () => {
    expect(refused(prepareSurface(ours([root(["v"]), video(SOUND)]), { files })).reason).toMatch(
      /not a video/,
    );
    expect(refused(prepareSurface(ours([root(["a"]), audio(CLIP)]), { files })).reason).toMatch(
      /not a sound/,
    );
    expect(refused(prepareSurface(ours([root(["v"]), video(PIC)]), { files })).rule).toBe(
      "artifact",
    );
    expect(refused(prepareSurface(ours([root(["v"]), video(CLIP)]))).rule).toBe("artifact");
  });

  it("a LinkPreview's link is checked like a card's: user information is refused (rule url)", () => {
    expect(
      refused(
        prepareSurface(
          ours([root(["l"]), link({ url: "https://evil.example@www.postgresql.org/" })]),
        ),
      ).rule,
    ).toBe("url");
    expect(
      refused(prepareSurface(ours([root(["l"]), link({ url: "javascript:alert(1)" })]))).rule,
    ).toBe("schema");
  });

  it("in the basic catalog none of them is drawn: they are not in its vocabulary", () => {
    const p = refused(
      prepareSurface(surface([root(["l"]), link()], undefined, "v0.9.1", BASIC_CATALOG)),
    );
    expect(p.rule).toBe("catalog");
  });
});
