/**
 * The files the mock server keeps (ADR 0032), the ones the orchestrator's `file` golden has: what
 * the real fake agent sends, byte for byte, so the mock's `artifact` event and its hash are the
 * golden's. A file is served as `GET /api/threads/{id}/artifacts/{sha256}` does: inline for a
 * preview type, as an attachment for `?download=1`, and always with the headers the API sends.
 */
import { createHash } from "node:crypto";
import { deflateSync } from "node:zlib";

/** A valid 1 x 1 PNG: the `PNG` of the orchestrator's fake agent. */
export const PNG = Buffer.from(
  "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415478" +
    "9c63f8cfc0f01f00050001ff89993d1d0000000049454e44ae426082",
  "hex",
);

export interface MockFile {
  bytes: Buffer;
  mimeType: string;
  filename: string;
  sha256: string;
}

function file(bytes: Buffer, mimeType: string, filename: string): MockFile {
  return { bytes, mimeType, filename, sha256: createHash("sha256").update(bytes).digest("hex") };
}

export const CHART = file(PNG, "image/png", "chart.png");

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (buf: Buffer): number => {
  let c = 0xffffffff;
  for (const byte of buf) c = (CRC_TABLE[(c ^ byte) & 0xff] as number) ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type: string, data: Buffer): Buffer => {
  const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const head = Buffer.alloc(4);
  head.writeUInt32BE(data.length);
  const tail = Buffer.alloc(4);
  tail.writeUInt32BE(crc32(body));
  return Buffer.concat([head, body, tail]);
};

/** A PNG of `width` x `height` pixels, each `[r, g, b]` from `pixel(x, y)` (8-bit RGB, no filter). */
export function png(
  width: number,
  height: number,
  pixel: (x: number, y: number) => [number, number, number],
): Buffer {
  const rows = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y++) {
    const at = y * (width * 3 + 1);
    for (let x = 0; x < width; x++) rows.set(pixel(x, y), at + 1 + x * 3);
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from("89504e470d0a1a0a", "hex"),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(rows)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/**
 * A bar chart a person can see (the golden's PNG is one pixel): five green bars on a light ground, the
 * results of a run, 480 x 240. What `files` and `file-image` keep.
 */
export const RESULTS = file(
  png(480, 240, (x, y) => {
    const bars = [150, 110, 170, 90, 130];
    const bar = Math.floor((x - 40) / 84);
    const inBar = x >= 40 && bar >= 0 && bar < bars.length && (x - 40) % 84 < 64;
    if (inBar && y >= 220 - (bars[bar] as number) && y < 220) return [0x3f, 0x9a, 0x5b];
    if (y >= 220 && y < 223 && x >= 30 && x < 460) return [0x55, 0x58, 0x55];
    if (x >= 30 && x < 33 && y >= 20 && y < 223) return [0x55, 0x58, 0x55];
    return [0xf3, 0xf2, 0xec];
  }),
  "image/png",
  "results.png",
);

/** A plain text file: a person can read it without downloading it (`preview: "text"`). */
export const NOTES = file(
  Buffer.from(
    [
      "Results of the run",
      "",
      "- login: 12 ms, 200",
      "- logout: 9 ms, 204",
      "- <script>alert(1)</script> is text here, never markup",
      "",
    ].join("\n"),
  ),
  "text/plain",
  "notes.txt",
);

/** An archive: nothing to look at, an attachment only (`preview: null`). */
export const EXPORT = file(
  Buffer.from("504b0506000000000000000000000000000000000000000000000000", "hex"),
  "application/zip",
  "export.zip",
);

/**
 * An SVG written to run something and to load something when it is looked at: a handler, a script,
 * a foreign object, a stylesheet and an image of another origin. The real API cleans an inline SVG
 * (`orch-svg-clean`); the mock serves it as it is, so the web's e2e shows that even the original,
 * drawn as an `<img>`, runs nothing and asks for nothing (ADR 0032, decision 9).
 */
export const HOSTILE_SVG = file(
  Buffer.from(
    [
      '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="120" height="60" viewBox="0 0 120 60" onload="window.__svgFlag = \'onload\'">',
      '<script>window.__svgFlag = "script"; fetch("https://evil.example/script")</script>',
      '<style>@import url("https://evil.example/style.css"); rect { fill: url("https://evil.example/fill") }</style>',
      '<rect width="120" height="60" fill="#3b6" onclick="window.__svgFlag = \'click\'"/>',
      '<image href="https://evil.example/image.png" width="10" height="10"/>',
      '<foreignObject width="100" height="50"><iframe xmlns="http://www.w3.org/1999/xhtml" src="https://evil.example/frame"></iframe></foreignObject>',
      '<use xlink:href="https://evil.example/use.svg#x"/>',
      '<text x="8" y="36" font-size="14" fill="#fff">a diagram</text>',
      "</svg>",
    ].join("\n"),
  ),
  "image/svg+xml",
  "diagram.svg",
);

/** The files of the mock, by hash (a thread holds them all: the mock has no per-thread store). */
export const FILES: ReadonlyMap<string, MockFile> = new Map(
  [CHART, RESULTS, NOTES, EXPORT, HOSTILE_SVG].map((f) => [f.sha256, f]),
);

/** `preview` of a kept file (`orch_core::Preview::of`): what a person can look at without downloading. */
export function previewOf(mimeType: string | undefined): "image" | "text" | null {
  switch (mimeType) {
    case "image/png":
    case "image/jpeg":
    case "image/gif":
    case "image/webp":
    case "image/svg+xml":
      return "image";
    case "text/plain":
    case "application/json":
      return "text";
    default:
      return null;
  }
}

/** The headers of a file response (`orch-api` `artifacts.rs`). */
export function fileHeaders(f: MockFile, download: boolean): Record<string, string> {
  const inline = !download && previewOf(f.mimeType) !== null;
  const textual = f.mimeType === "text/plain" || f.mimeType === "application/json";
  return {
    "Content-Type": inline && textual ? `${f.mimeType}; charset=utf-8` : f.mimeType,
    "Content-Disposition": `${inline ? "inline" : "attachment"}; filename="${f.filename}"`,
    "Content-Length": String(f.bytes.length),
    "X-Content-Type-Options": "nosniff",
    "Content-Security-Policy":
      "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox",
    "Cache-Control": "private, max-age=31536000, immutable",
    ETag: `"${f.sha256}"`,
  };
}
