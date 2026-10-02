import { describe, expect, it, vi } from "vitest";
import { ACTIVITY, activityPartName, parseArtifact } from "./agui/vymalo";
import {
  downloadHref,
  fileAlt,
  filesOf,
  fileTitle,
  formatSize,
  GENERIC_ALT,
  keptFileOf,
  plainName,
  readTextPreview,
  sameFiles,
  TEXT_PREVIEW_BYTES,
} from "./files";

const SHA = "a".repeat(64);
const OTHER = "b".repeat(64);
const HREF = `/api/threads/t-1/artifacts/${SHA}`;

/** The `vymalo.artifact` content of a file the store kept (docs/api/examples/agui/file.agui.json). */
const kept = (over: Record<string, unknown> = {}) => ({
  kind: "file",
  name: "chart",
  mimeType: "image/png",
  href: HREF,
  sha256: SHA,
  size: 70,
  filename: "chart.png",
  preview: "image",
  ...over,
});

const artifactOf = (over: Record<string, unknown> = {}) => {
  const a = parseArtifact(kept(over));
  if (!a) throw new Error("not an artifact");
  return a;
};

describe("a kept file, read from vymalo.artifact", () => {
  it("is the reference the projection sent: where, how big, what it is called, how it previews", () => {
    expect(keptFileOf(artifactOf())).toEqual({
      sha256: SHA,
      href: HREF,
      size: 70,
      filename: "chart.png",
      name: "chart",
      mimeType: "image/png",
      preview: "image",
    });
  });

  it("a preview that is null (an attachment only) or one this build does not know is no preview", () => {
    expect(keptFileOf(artifactOf({ preview: null }))?.preview).toBeNull();
    expect(keptFileOf(artifactOf({ preview: "video" }))?.preview).toBeNull();
    const { preview: _gone, ...without } = kept();
    expect(keptFileOf(parseArtifact(without) as never)?.preview).toBeNull();
  });

  it("the file name is optional", () => {
    const file = keptFile({ filename: undefined });
    expect(file).not.toHaveProperty("filename");
    expect(fileTitle(file)).toBe("chart");
    expect(fileAlt(file)).toBe(GENERIC_ALT);
  });

  it("a file that was not kept (no href) is no kept file, and neither is a link, a pull request or a half reference", () => {
    for (const over of [
      { href: undefined, sha256: undefined, size: undefined, preview: undefined },
      { href: undefined },
      { sha256: undefined },
      { size: undefined },
    ]) {
      expect(keptFileOf(artifactOf(over)), JSON.stringify(over)).toBeUndefined();
    }
    expect(keptFileOf(artifactOf({ kind: "pull_request" }))).toBeUndefined();
    expect(keptFileOf(artifactOf({ kind: "branch" }))).toBeUndefined();
  });

  it("is fetched from the API's route and nothing else: any other href is dropped", () => {
    for (const href of [
      `https://evil.example/api/threads/t-1/artifacts/${SHA}`,
      `//evil.example/api/threads/t-1/artifacts/${SHA}`,
      `/api/threads/t-1/artifacts/${SHA}?x=1`,
      `/api/threads/t-1/artifacts/${SHA}/../x`,
      `/api/threads/../artifacts/${SHA}`,
      `/api/threads/t-1/artifacts/${SHA.toUpperCase()}`,
      "javascript:alert(1)",
      "data:image/png;base64,AAAA",
      "/api/threads/t-1/artifacts/abc",
    ]) {
      expect(keptFileOf(artifactOf({ href })), href).toBeUndefined();
    }
  });

  it("the hash in the href is the hash of the file: a mismatch is no kept file", () => {
    expect(keptFileOf(artifactOf({ sha256: OTHER }))).toBeUndefined();
  });

  it("a size that is not a count of bytes is no kept file", () => {
    for (const size of [-1, 1.5, "70", Number.NaN, Number.MAX_SAFE_INTEGER + 1]) {
      expect(keptFileOf(artifactOf({ size })), String(size)).toBeUndefined();
    }
    expect(keptFileOf(artifactOf({ size: 0 }))?.size).toBe(0);
  });

  it("never reads the agent's uri: the card is the href's, whatever the uri says", () => {
    const a = artifactOf({ uri: "https://evil.example/x.png" });
    expect(keptFileOf(a)?.href).toBe(HREF);
  });

  it("downloads from the same route with ?download=1", () => {
    expect(downloadHref({ href: HREF })).toBe(`${HREF}?download=1`);
  });
});

function keptFile(over: Record<string, unknown> = {}) {
  const file = keptFileOf(artifactOf(over));
  if (!file) throw new Error("not kept");
  return file;
}

describe("names and sizes", () => {
  it("a name is one line of text: no control, newline or direction characters", () => {
    expect(plainName("a\u0000b\nc‮d.png")).toBe("abcd.png");
    expect(plainName("  report.pdf  ")).toBe("report.pdf");
    expect(plainName("x".repeat(300))).toHaveLength(120);
    expect(plainName("x".repeat(300)).endsWith("…")).toBe(true);
    expect(plainName("‮\u0007")).toBe("");
  });

  it("the title is the file name, else the artifact's name, else a word", () => {
    expect(fileTitle({ filename: "a.png", name: "chart" })).toBe("a.png");
    expect(fileTitle({ name: "chart" })).toBe("chart");
    expect(fileTitle({ filename: "\u0007", name: "" })).toBe("File");
  });

  it("the alt text is the file name, else a generic sentence", () => {
    expect(fileAlt({ filename: "chart.png" })).toBe("chart.png");
    expect(fileAlt({})).toBe("File from the agent");
  });

  it("a size a person reads", () => {
    expect(formatSize(0)).toBe("0 bytes");
    expect(formatSize(1)).toBe("1 byte");
    expect(formatSize(70)).toBe("70 bytes");
    expect(formatSize(1023)).toBe("1023 bytes");
    expect(formatSize(1024)).toBe("1.0 KB");
    expect(formatSize(1536)).toBe("1.5 KB");
    expect(formatSize(10 * 1024 * 1024)).toBe("10.0 MB");
    expect(formatSize(150 * 1024)).toBe("150 KB");
    expect(formatSize(3 * 1024 ** 3)).toBe("3.0 GB");
    expect(formatSize(-1)).toBe("");
    expect(formatSize(Number.NaN)).toBe("");
  });
});

describe("the files of a thread", () => {
  const part = (content: Record<string, unknown>) => ({
    type: "data",
    name: activityPartName(ACTIVITY.artifact),
    data: content,
  });
  const agent = (...content: ReturnType<typeof part>[]) => ({ role: "assistant", content });

  it("are every kept file the agents handed over, each hash once, in order", () => {
    const other = { sha256: OTHER, href: `/api/threads/t-1/artifacts/${OTHER}`, filename: "b.txt" };
    const files = filesOf([
      { role: "user", content: [part(kept({ filename: "mine.png" }))] },
      agent(part(kept()), part({ kind: "file", name: "not kept" })),
      agent(part(kept({ filename: "again.png" })), part(kept(other))),
    ]);
    expect(files.map((f) => [f.sha256, f.filename])).toEqual([
      [SHA, "chart.png"],
      [OTHER, "b.txt"],
    ]);
  });

  it("none for a thread without any", () => {
    expect(filesOf([])).toEqual([]);
    expect(filesOf([agent(part({ kind: "pull_request", name: "pr" }))])).toEqual([]);
  });

  it("a list is the same while the files are", () => {
    const a = filesOf([agent(part(kept()))]);
    const b = filesOf([agent(part(kept())), agent(part({ kind: "file", name: "x" }))]);
    expect(sameFiles(a, b)).toBe(true);
    expect(sameFiles(a, [])).toBe(false);
  });
});

describe("the preview of a text file", () => {
  const response = (chunks: Uint8Array[], status = 200) =>
    new Response(
      new ReadableStream({
        start(controller) {
          for (const c of chunks) controller.enqueue(c);
          controller.close();
        },
      }),
      { status },
    );
  const fetcher = (res: Response) => vi.fn(async () => res) as unknown as typeof fetch;
  const bytes = (s: string) => new TextEncoder().encode(s);

  it("reads the whole of a small file, as text", async () => {
    const f = fetcher(response([bytes("hello "), bytes("wörld\n")]));
    expect(await readTextPreview(HREF, f)).toEqual({ text: "hello wörld\n", truncated: false });
    expect(f).toHaveBeenCalledWith(HREF, { credentials: "same-origin" });
  });

  it("stops at the cap, says it did, and does not read the rest", async () => {
    const big = new Uint8Array(TEXT_PREVIEW_BYTES + 5000).fill(97);
    const preview = await readTextPreview(HREF, fetcher(response([big])));
    expect(preview.truncated).toBe(true);
    expect(preview.text).toHaveLength(TEXT_PREVIEW_BYTES);
  });

  it("a file of exactly the cap is not truncated", async () => {
    const exact = new Uint8Array(TEXT_PREVIEW_BYTES).fill(98);
    const preview = await readTextPreview(HREF, fetcher(response([exact])));
    expect(preview).toMatchObject({ truncated: false });
    expect(preview.text).toHaveLength(TEXT_PREVIEW_BYTES);
  });

  it("an invalid sequence is a replacement character, not an error", async () => {
    const preview = await readTextPreview(
      HREF,
      fetcher(response([new Uint8Array([104, 0xff, 105])])),
    );
    expect(preview.text).toBe("h�i");
  });

  it("an answer that is not 200 is an error", async () => {
    await expect(readTextPreview(HREF, fetcher(response([], 404)))).rejects.toThrow(/404/);
  });
});
