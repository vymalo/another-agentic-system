import { describe, expect, it } from "vitest";
import lock from "./catalog.lock.json";
import { componentNames, OWN_CATALOG } from "./index";
import { compileCatalog } from "./validate";

/**
 * Catalog version 6, group 4 (media): Gallery, LinkPreview, Video and Audio. Each schema on its own: valid, missing,
 * oversize and wrong types. The rules JSON Schema cannot say (a picture is an image of the thread, a video a video)
 * are in prepare-v6-media.test.ts.
 */

const check = (component: string, instance: unknown) =>
  compileCatalog(OWN_CATALOG.catalog).check(component, instance);
const SHA = "a1".repeat(32);
const text = (n: number) => "x".repeat(n);

describe("version 6: media", () => {
  it("is catalog version 6, and has the four components", () => {
    expect(OWN_CATALOG.version).toBe(6);
    expect(lock.version).toBe(6);
    expect(componentNames(OWN_CATALOG.catalog)).toEqual(
      expect.arrayContaining(["Gallery", "LinkPreview", "Video", "Audio"]),
    );
  });

  it("keeps the first sentence of each description short enough for adam's show to list it whole", () => {
    for (const name of ["Gallery", "LinkPreview", "Video", "Audio"]) {
      const d = (OWN_CATALOG.catalog.components[name] as { description: string }).description;
      const end = d.indexOf(". ");
      const first = end < 0 ? d : d.slice(0, end + 1);
      expect(first.length, name).toBeLessThanOrEqual(100);
    }
  });
});

describe("Gallery", () => {
  const gallery = (
    images: unknown[] = [{ artifact: SHA, alt: "A chart" }],
    over: Record<string, unknown> = {},
  ) => ({
    id: "g",
    component: "Gallery",
    images,
    ...over,
  });

  it("accepts pictures of a hash and an alt text, with a caption and a title", () => {
    expect(check("Gallery", gallery())).toBeUndefined();
    expect(
      check(
        "Gallery",
        gallery([{ artifact: SHA, alt: "A chart", caption: "Figure 1" }], { title: "Runs" }),
      ),
    ).toBeUndefined();
  });

  it("needs at least one picture, and each picture its hash and its alt text", () => {
    expect(check("Gallery", gallery([]))).toMatch(/^images: /);
    expect(check("Gallery", gallery([{ alt: "a" }]))).toMatch(/required property "artifact"/);
    expect(check("Gallery", gallery([{ artifact: SHA }]))).toMatch(/required property "alt"/);
    expect(check("Gallery", { id: "g", component: "Gallery" })).toMatch(
      /required property "images"/,
    );
  });

  it("a picture names a file by its sha256, and never a URL", () => {
    for (const bad of ["A1".repeat(32), "a1".repeat(31), "https://example.com/a.png", "/api/x"]) {
      expect(check("Gallery", gallery([{ artifact: bad, alt: "a" }])), bad).toMatch(
        /^images\.0\.artifact: /,
      );
    }
    expect(check("Gallery", gallery([{ artifact: SHA, alt: "a", src: "x" }]))).toMatch(/src/);
  });

  it("limits: 12 pictures; the alt text 300 characters; the caption 300", () => {
    const twelve = Array.from({ length: 12 }, () => ({ artifact: SHA, alt: "a" }));
    expect(check("Gallery", gallery(twelve))).toBeUndefined();
    expect(check("Gallery", gallery([...twelve, { artifact: SHA, alt: "a" }]))).toMatch(
      /^images: /,
    );
    expect(check("Gallery", gallery([{ artifact: SHA, alt: text(300) }]))).toBeUndefined();
    expect(check("Gallery", gallery([{ artifact: SHA, alt: text(301) }]))).toMatch(
      /^images\.0\.alt: /,
    );
    expect(check("Gallery", gallery([{ artifact: SHA, alt: "a", caption: text(301) }]))).toMatch(
      /^images\.0\.caption: /,
    );
  });
});

describe("LinkPreview", () => {
  const link = (over: Record<string, unknown> = {}) => ({
    id: "l",
    component: "LinkPreview",
    url: "https://www.postgresql.org/docs/current/",
    title: "PostgreSQL documentation",
    ...over,
  });

  it("accepts a link with a title, and with a description and a site", () => {
    expect(check("LinkPreview", link())).toBeUndefined();
    expect(
      check(
        "LinkPreview",
        link({ description: "The manual of the current release.", site: "postgresql.org" }),
      ),
    ).toBeUndefined();
  });

  it("needs its url and its title", () => {
    const { url: _url, ...noUrl } = link();
    const { title: _title, ...noTitle } = link();
    expect(check("LinkPreview", noUrl)).toMatch(/required property "url"/);
    expect(check("LinkPreview", noTitle)).toMatch(/required property "title"/);
  });

  it("a link is an http(s) URL of at most 2,048 characters, and nothing else", () => {
    expect(check("LinkPreview", link({ url: "http://example.com/" }))).toBeUndefined();
    for (const bad of ["javascript:alert(1)", "data:text/html,x", "/relative", "ftp://x"]) {
      expect(check("LinkPreview", link({ url: bad })), bad).toMatch(/^url: /);
    }
    expect(
      check("LinkPreview", link({ url: `https://e.example/${"p".repeat(2048 - 18)}` })),
    ).toBeUndefined();
    expect(
      check("LinkPreview", link({ url: `https://e.example/${"p".repeat(2049 - 18)}` })),
    ).toMatch(/^url: /);
    expect(check("LinkPreview", link({ image: "https://example.com/a.png" }))).toMatch(/image/);
  });

  it("limits: a title of 200, a description of 1,000, a site of 100", () => {
    expect(check("LinkPreview", link({ title: text(200) }))).toBeUndefined();
    expect(check("LinkPreview", link({ title: text(201) }))).toMatch(/^title: /);
    expect(check("LinkPreview", link({ description: text(1000) }))).toBeUndefined();
    expect(check("LinkPreview", link({ description: text(1001) }))).toMatch(/^description: /);
    expect(check("LinkPreview", link({ site: text(101) }))).toMatch(/^site: /);
    expect(check("LinkPreview", link({ site: "" }))).toMatch(/^site: /);
  });
});

describe("Video and Audio", () => {
  const media = (component: "Video" | "Audio", over: Record<string, unknown> = {}) => ({
    id: "m",
    component,
    artifact: SHA,
    title: "The demo",
    ...over,
  });

  it("accept a file by its sha256, a title, and a caption", () => {
    for (const component of ["Video", "Audio"] as const) {
      expect(check(component, media(component)), component).toBeUndefined();
      expect(
        check(component, media(component, { caption: "Recorded on Tuesday." })),
        component,
      ).toBeUndefined();
    }
  });

  it("need the file and the title; a file is a sha256 and never a URL", () => {
    for (const component of ["Video", "Audio"] as const) {
      const { artifact: _a, ...noFile } = media(component);
      const { title: _t, ...noTitle } = media(component);
      expect(check(component, noFile), component).toMatch(/required property "artifact"/);
      expect(check(component, noTitle), component).toMatch(/required property "title"/);
      expect(
        check(component, media(component, { artifact: "https://example.com/a.mp4" })),
        component,
      ).toMatch(/^artifact: /);
      expect(
        check(component, media(component, { src: "https://example.com/a.mp4" })),
        component,
      ).toMatch(/src/);
    }
  });

  it("limits: a title of 200 characters, a caption of 500", () => {
    for (const component of ["Video", "Audio"] as const) {
      expect(check(component, media(component, { title: text(200) }))).toBeUndefined();
      expect(check(component, media(component, { title: text(201) }))).toMatch(/^title: /);
      expect(check(component, media(component, { caption: text(501) }))).toMatch(/^caption: /);
    }
  });

  it("the descriptions say a file of this thread, and that it is played in the page", () => {
    expect((OWN_CATALOG.catalog.components.Video as { description: string }).description).toMatch(
      /this thread/,
    );
    expect((OWN_CATALOG.catalog.components.Audio as { description: string }).description).toMatch(
      /this thread/,
    );
  });
});
