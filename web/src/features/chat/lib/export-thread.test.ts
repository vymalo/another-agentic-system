import { describe, expect, it } from "vitest";
import { exportFilename } from "./export-thread";

const ID = "0190aaaa-0000-7000-8000-000000000123";

describe("exportFilename", () => {
  it("takes the plain name the server gave", () => {
    expect(exportFilename(ID, `attachment; filename="thread-${ID}.json"`)).toBe(
      `thread-${ID}.json`,
    );
  });

  it("names the file after the thread when the header is missing or says nothing usable", () => {
    const fallback = `thread-${ID}.json`;
    expect(exportFilename(ID, null)).toBe(fallback);
    expect(exportFilename(ID, "attachment")).toBe(fallback);
    expect(exportFilename(ID, 'attachment; filename=""')).toBe(fallback);
  });

  it("never lets the server choose a path or a hidden file", () => {
    const fallback = `thread-${ID}.json`;
    expect(exportFilename(ID, 'attachment; filename="../../etc/passwd"')).toBe(fallback);
    expect(exportFilename(ID, 'attachment; filename="a/b.json"')).toBe(fallback);
    expect(exportFilename(ID, 'attachment; filename=".bashrc"')).toBe(fallback);
    expect(exportFilename(ID, 'attachment; filename="a b.json"')).toBe(fallback);
  });
});
