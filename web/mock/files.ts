/**
 * The files the mock server keeps (ADR 0032), the ones the orchestrator's `file` golden has: what
 * the real fake agent sends, byte for byte, so the mock's `artifact` event and its hash are the
 * golden's. A file is served as `GET /api/threads/{id}/artifacts/{sha256}` does: inline for a
 * preview type, as an attachment for `?download=1`, and always with the headers the API sends.
 */
import { createHash } from "node:crypto";

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

/** The files of the mock, by hash (a thread holds them all: the mock has no per-thread store). */
export const FILES: ReadonlyMap<string, MockFile> = new Map([[CHART.sha256, CHART]]);

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
