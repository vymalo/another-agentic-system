import { describe, expect, it } from "vitest";
import { ACTIVITY, activityPartName } from "./agui/vymalo";
import {
  fileSource,
  hashesOf,
  imageSourcesIn,
  inlineFileHashes,
  normalisePath,
  resolveSharedImage,
  type SharedFile,
  sharedFilesOf,
} from "./inline-images";

const sha = (c: string) => c.repeat(64);

const part = (type: string, value: Record<string, unknown>) => ({
  type: "data",
  name: activityPartName(type),
  data: value,
});

/** The share step of the coder: `share_file { path, name, repo }`, reported once. */
const shareStep = (id: string, input: Record<string, unknown> | undefined) =>
  part(ACTIVITY.step, {
    id,
    path: [],
    kind: "tool",
    label: "Share a file",
    state: "completed",
    ...(input ? { input } : {}),
  });

/** The artifact event the file became: `kind: file`, with the reference the projection made. */
const sharedArtifact = (hash: string, name: string, over: Record<string, unknown> = {}) =>
  part(ACTIVITY.artifact, {
    kind: "file",
    name,
    mimeType: "image/png",
    href: `/api/threads/t-1/artifacts/${hash}`,
    sha256: hash,
    size: 1200,
    filename: name,
    preview: "image",
    ...over,
  });

const message = (id: string, content: ReturnType<typeof part>[], role = "assistant") => ({
  id,
  role,
  content,
});

describe("the files an agent shared, with the path each was shared from", () => {
  it("pairs a share step with the artifact it made, by the name the step gave it", () => {
    const shared = sharedFilesOf([
      message("m1", [
        shareStep("s1", { path: "shots/4-matches.png", name: "4-matches.png", repo: "demo" }),
        sharedArtifact(sha("a"), "4-matches.png"),
      ]),
    ]);
    expect(shared).toHaveLength(1);
    expect(shared[0]).toMatchObject({
      path: "shots/4-matches.png",
      run: "m1",
      file: { sha256: sha("a"), filename: "4-matches.png", preview: "image" },
    });
  });

  it("falls back to the base name of the path when the step gave no name", () => {
    const [one] = sharedFilesOf([
      message("m1", [
        shareStep("s1", { path: "out/chart.png" }),
        sharedArtifact(sha("a"), "chart.png"),
      ]),
    ]);
    expect(one?.path).toBe("out/chart.png");
  });

  it("pairs steps and artifacts that arrive in another order, one step per artifact", () => {
    const shared = sharedFilesOf([
      message("m1", [
        shareStep("s1", { path: "shots/1.png", name: "1.png" }),
        shareStep("s2", { path: "shots/2.png", name: "2.png" }),
        sharedArtifact(sha("b"), "2.png"),
        sharedArtifact(sha("a"), "1.png"),
      ]),
    ]);
    expect(shared.map((s) => [s.file.filename, s.path])).toEqual([
      ["2.png", "shots/2.png"],
      ["1.png", "shots/1.png"],
    ]);
  });

  it("pairs a step that was first reported after its artifact", () => {
    const [one] = sharedFilesOf([
      message("m1", [
        sharedArtifact(sha("a"), "4-matches.png"),
        shareStep("s1", { path: "shots/4-matches.png", name: "4-matches.png" }),
      ]),
    ]);
    expect(one?.path).toBe("shots/4-matches.png");
  });

  it("a step is taken by one artifact, the nearest before it", () => {
    const shared = sharedFilesOf([
      message("m1", [
        shareStep("s1", { path: "a/list.png", name: "list.png" }),
        sharedArtifact(sha("a"), "list.png"),
        shareStep("s2", { path: "b/list.png", name: "list.png" }),
        sharedArtifact(sha("b"), "list.png"),
      ]),
    ]);
    expect(shared.map((s) => s.path)).toEqual(["a/list.png", "b/list.png"]);
  });

  it("a step reported again (it started, then ended) is one step with the later input", () => {
    const shared = sharedFilesOf([
      message("m1", [
        shareStep("s1", { path: "shots/1.png", name: "1.png" }),
        sharedArtifact(sha("a"), "1.png"),
        shareStep("s1", { path: "shots/1.png", name: "1.png", repo: "demo" }),
      ]),
    ]);
    expect(shared).toHaveLength(1);
    expect(shared[0]?.path).toBe("shots/1.png");
  });

  it("a file whose step has no path (its input was cut) does not take the next share's step", () => {
    const shared = sharedFilesOf([
      message("m1", [
        part(ACTIVITY.step, {
          id: "s0",
          path: [],
          kind: "tool",
          label: "Share a file",
          state: "completed",
          input: { _cut: true, bytes: 9000 },
        }),
        sharedArtifact(sha("a"), "list.png"),
        shareStep("s1", { path: "shots/list.png", name: "list.png" }),
        sharedArtifact(sha("b"), "list.png"),
      ]),
    ]);
    expect(shared.map((s) => [s.file.sha256, s.path])).toEqual([
      [sha("a"), undefined],
      [sha("b"), "shots/list.png"],
    ]);
    expect(resolveSharedImage("shots/list.png", shared)?.sha256).toBe(sha("b"));
  });

  it("looks after a file only for the steps no file before them took", () => {
    const shared = sharedFilesOf([
      message("m1", [
        shareStep("s1", { path: "a/1.png", name: "1.png" }),
        sharedArtifact(sha("a"), "1.png"),
        sharedArtifact(sha("b"), "2.png"),
        shareStep("s2", { path: "b/2.png", name: "2.png" }),
        shareStep("s3", { path: "c/1.png", name: "1.png" }),
      ]),
    ]);
    expect(shared.map((s) => s.path)).toEqual(["a/1.png", "b/2.png"]);
  });

  it("a file with no step before it (a plain A2A agent, an input the job had no room for) has no path", () => {
    const shared = sharedFilesOf([
      message("m1", [
        shareStep("s1", undefined),
        sharedArtifact(sha("a"), "a.png"),
        sharedArtifact(sha("b"), "b.png"),
      ]),
    ]);
    expect(shared.map((s) => s.path)).toEqual([undefined, undefined]);
  });

  it("does not pair an artifact with the step of another message, nor read the person's messages", () => {
    const shared = sharedFilesOf([
      message(
        "u1",
        [shareStep("s0", { path: "x/a.png" }), sharedArtifact(sha("c"), "a.png")],
        "user",
      ),
      message("m1", [shareStep("s1", { path: "x/a.png" })]),
      message("m2", [sharedArtifact(sha("a"), "a.png")]),
    ]);
    expect(shared).toHaveLength(1);
    expect(shared[0]).toMatchObject({ run: "m2" });
    expect(shared[0]?.path).toBeUndefined();
  });

  it("ignores what is not a kept file: a link, a pull request, a file the store did not keep", () => {
    const shared = sharedFilesOf([
      message("m1", [
        part(ACTIVITY.artifact, { kind: "file", name: "dump", mimeType: "image/png" }),
        part(ACTIVITY.artifact, {
          kind: "pull_request",
          name: "pr",
          url: "https://github.com/a/b/pull/1",
        }),
        sharedArtifact(sha("a"), "a.png"),
      ]),
    ]);
    expect(shared.map((s) => s.file.sha256)).toEqual([sha("a")]);
  });
});

/** A shared file as the resolver reads it. */
const shared = (
  hash: string,
  filename: string,
  over: {
    path?: string;
    run?: string;
    index?: number;
    preview?: "image" | "text" | null;
    name?: string;
  } = {},
): SharedFile => ({
  run: over.run ?? "m1",
  index: over.index ?? 0,
  ...(over.path !== undefined ? { path: over.path } : {}),
  file: {
    sha256: hash,
    href: `/api/threads/t-1/artifacts/${hash}`,
    size: 10,
    filename,
    name: over.name ?? filename,
    preview: over.preview === undefined ? "image" : over.preview,
  },
});

describe("resolving an image of an answer against the shared files", () => {
  it("finds the file whose share step had exactly that path", () => {
    const files = [
      shared(sha("a"), "4-matches.png", { path: "shots/4-matches.png" }),
      shared(sha("b"), "4-matches.png", { path: "other/4-matches.png" }),
    ];
    expect(resolveSharedImage("shots/4-matches.png", files)?.sha256).toBe(sha("a"));
    expect(resolveSharedImage("other/4-matches.png", files)?.sha256).toBe(sha("b"));
  });

  it("takes a leading ./ and the query or fragment of the source as no part of the path", () => {
    const files = [shared(sha("a"), "4.png", { path: "shots/4.png" })];
    expect(resolveSharedImage("./shots/4.png", files)?.sha256).toBe(sha("a"));
    expect(resolveSharedImage("shots/4.png?raw=1#top", files)?.sha256).toBe(sha("a"));
  });

  it("reads a percent-encoded source as the path it spells", () => {
    const files = [shared(sha("a"), "my shot.png", { path: "shots/my shot.png" })];
    expect(resolveSharedImage("shots/my%20shot.png", files)?.sha256).toBe(sha("a"));
    // a source that is not valid percent-encoding is read as it is, and found by nothing
    expect(resolveSharedImage("shots/%E0%A4%A.png", files)).toBeUndefined();
  });

  it("compares the step's path and the source in one spelling: escapes, ./, query and fragment on either side", () => {
    const files = [
      shared(sha("a"), "my shot.png", { path: "./shots/my shot.png" }),
      shared(sha("b"), "é.png", { path: "shots/é.png?v=1" }),
    ];
    // what the renderer hands over: spaces and non-ASCII escaped
    expect(resolveSharedImage("shots/my%20shot.png", files)?.sha256).toBe(sha("a"));
    expect(resolveSharedImage("./shots/my%20shot.png?raw=1#top", files)?.sha256).toBe(sha("a"));
    expect(resolveSharedImage("shots/%C3%A9.png", files)?.sha256).toBe(sha("b"));
    expect(resolveSharedImage("shots/é.png#x", files)?.sha256).toBe(sha("b"));
  });

  it("an exact path beats the base name even when the source is escaped or has a query, with a same-named file shared last", () => {
    const files = [
      shared(sha("a"), "chart.png", { path: "a/chart.png" }),
      shared(sha("b"), "chart.png", { path: "b/chart.png" }),
      shared(sha("c"), "chart.png", { path: "c/chart.png" }),
    ];
    expect(resolveSharedImage("./a/chart.png?x=1#y", files)?.sha256).toBe(sha("a"));
    expect(resolveSharedImage("b/chart.png", files)?.sha256).toBe(sha("b"));
    // no path says it: the latest of the name
    expect(resolveSharedImage("chart.png", files)?.sha256).toBe(sha("c"));
  });

  it("otherwise matches the base name of the source against the file name", () => {
    const files = [shared(sha("a"), "4-matches.png", { path: "shots/4-matches.png" })];
    expect(resolveSharedImage("/work/demo/shots/4-matches.png", files)?.sha256).toBe(sha("a"));
    expect(resolveSharedImage("4-matches.png", files)?.sha256).toBe(sha("a"));
  });

  it("matches the base name against the artifact's name when its file name does not say it", () => {
    const files = [shared(sha("a"), "x.png", { name: "overview.png" })];
    expect(resolveSharedImage("figures/overview.png", files)?.sha256).toBe(sha("a"));
  });

  it("an exact path beats a base name that happens to match another file", () => {
    const files = [
      shared(sha("a"), "list.png", { path: "shots/list.png" }),
      shared(sha("b"), "list.png", { path: "old/list.png" }),
    ];
    expect(resolveSharedImage("shots/list.png", files)?.sha256).toBe(sha("a"));
  });

  it("is nothing when no file matches, and for a source that names no file", () => {
    const files = [shared(sha("a"), "4-matches.png", { path: "shots/4-matches.png" })];
    expect(resolveSharedImage("shots/unknown.png", files)).toBeUndefined();
    expect(resolveSharedImage("", files)).toBeUndefined();
    expect(resolveSharedImage("shots/", files)).toBeUndefined();
    expect(resolveSharedImage("shots/4-matches.png", [])).toBeUndefined();
  });

  it("the same name shared twice is the latest share, by path and by name", () => {
    const files = [
      shared(sha("a"), "list.png", { path: "shots/list.png" }),
      shared(sha("b"), "list.png", { path: "shots/list.png" }),
    ];
    expect(resolveSharedImage("shots/list.png", files)?.sha256).toBe(sha("b"));
    expect(resolveSharedImage("list.png", files)?.sha256).toBe(sha("b"));
  });

  it("prefers a file of the same run over a later one of another run", () => {
    const files = [
      shared(sha("a"), "list.png", { path: "shots/list.png", run: "m1", index: 0 }),
      shared(sha("b"), "list.png", { path: "shots/list.png", run: "m2", index: 1 }),
    ];
    expect(resolveSharedImage("shots/list.png", files, { run: "m1", index: 0 })?.sha256).toBe(
      sha("a"),
    );
    expect(resolveSharedImage("shots/list.png", files, { run: "m2", index: 1 })?.sha256).toBe(
      sha("b"),
    );
    // a run that shared nothing of that name finds the latest share before it
    expect(resolveSharedImage("shots/list.png", files, { run: "m3", index: 2 })?.sha256).toBe(
      sha("b"),
    );
  });

  it("a file of the same run found by its name beats another run's exact path", () => {
    const files = [
      shared(sha("a"), "list.png", { path: "shots/list.png", run: "m1", index: 0 }),
      shared(sha("b"), "list.png", { run: "m2", index: 1 }),
    ];
    expect(resolveSharedImage("shots/list.png", files, { run: "m2", index: 1 })?.sha256).toBe(
      sha("b"),
    );
  });

  it("a turn cannot show a file that was shared after it", () => {
    const files = [shared(sha("a"), "list.png", { path: "shots/list.png", run: "m2", index: 1 })];
    expect(resolveSharedImage("shots/list.png", files, { run: "m1", index: 0 })).toBeUndefined();
    expect(resolveSharedImage("shots/list.png", files, { run: "m3", index: 2 })?.sha256).toBe(
      sha("a"),
    );
  });

  it("never resolves a file that is not an image", () => {
    const files = [
      shared(sha("a"), "notes.txt", { path: "out/notes.txt", preview: "text" }),
      shared(sha("b"), "export.zip", { path: "out/export.zip", preview: null }),
    ];
    expect(resolveSharedImage("out/notes.txt", files)).toBeUndefined();
    expect(resolveSharedImage("export.zip", files)).toBeUndefined();
  });

  it("a non-image does not hide an image of the same name", () => {
    const files = [
      shared(sha("a"), "report.png", { path: "out/report.png" }),
      shared(sha("b"), "report.png", { path: "out/report.png", preview: null }),
    ];
    expect(resolveSharedImage("out/report.png", files)?.sha256).toBe(sha("a"));
  });

  it("never resolves a source that names a place: a scheme, or the network path of another host", () => {
    const files = [shared(sha("a"), "4-matches.png", { path: "shots/4-matches.png" })];
    for (const src of [
      "https://evil.example/4-matches.png",
      "http://evil.example/4-matches.png",
      "data:image/png;base64,AAAA",
      "file:///work/shots/4-matches.png",
      "javascript:alert(1)//4-matches.png",
      "//evil.example/4-matches.png",
      "C:\\work\\4-matches.png",
    ]) {
      expect(fileSource(src), src).toBeUndefined();
      expect(resolveSharedImage(src, files), src).toBeUndefined();
    }
  });
});

describe("normalisePath", () => {
  it("is one spelling of a path", () => {
    expect(normalisePath("./shots/a b.png?x=1#y")).toBe("shots/a b.png");
    expect(normalisePath("shots/a%20b.png")).toBe("shots/a b.png");
    expect(normalisePath("././a.png")).toBe("a.png");
    // an escaped # or ? is a character of the name, not the start of a fragment
    expect(normalisePath("shots/chart%231.png")).toBe("shots/chart#1.png");
    expect(normalisePath("shots/%E0%A4%A.png")).toBe("shots/%E0%A4%A.png");
  });
});

describe("the sources of the images of a markdown text", () => {
  it("lists inline images and reference images, in order", () => {
    const md = [
      "![One](shots/1.png)",
      "",
      "Text ![Two][two] and ![](<shots/with space.png> 'title')",
      "",
      "[two]: shots/2.png",
    ].join("\n");
    expect(imageSourcesIn(md)).toEqual(["shots/1.png", "shots/2.png", "shots/with space.png"]);
  });

  it("is not fooled by an image written in code, or by a link", () => {
    const md = ["`![x](a.png)`", "", "```md", "![y](b.png)", "```", "", "[link](c.png)"].join("\n");
    expect(imageSourcesIn(md)).toEqual([]);
  });

  it("is quick about a text with no image at all", () => {
    expect(imageSourcesIn("just words")).toEqual([]);
  });
});

describe("the files an answer shows inline", () => {
  const files = [
    shared(sha("a"), "1.png", { path: "shots/1.png" }),
    shared(sha("b"), "2.png", { path: "shots/2.png" }),
    shared(sha("c"), "3.png", { path: "shots/3.png" }),
  ];

  it("is the hash of every image that resolves; a file nothing references is not in it", () => {
    const hashes = inlineFileHashes("![a](shots/1.png) and ![b](shots/2.png)", files);
    expect([...hashes].sort()).toEqual([sha("a"), sha("b")]);
    expect(hashes.has(sha("c"))).toBe(false);
  });

  it("an image that does not resolve, or points at a remote URL, hides no file", () => {
    const hashes = inlineFileHashes(
      "![x](shots/none.png) ![y](https://example.com/1.png) ![z](//evil.example/1.png)",
      files,
    );
    expect(hashes.size).toBe(0);
  });

  it("hashesOf takes the sources already read, so the text is parsed once", () => {
    expect([...hashesOf(["shots/1.png", "shots/none.png"], files)]).toEqual([sha("a")]);
  });

  it("resolves for the turn it is asked for", () => {
    const two = [
      shared(sha("a"), "1.png", { path: "shots/1.png", run: "m1", index: 0 }),
      shared(sha("b"), "1.png", { path: "shots/1.png", run: "m2", index: 1 }),
    ];
    const asked = { run: "m1", index: 0 };
    expect([...inlineFileHashes("![x](shots/1.png)", two, asked)]).toEqual([sha("a")]);
  });
});
