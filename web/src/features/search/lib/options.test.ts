import { describe, expect, it } from "vitest";
import type { ApiThread, ApiThreadHit } from "@/lib/api/types";
import { clip, hrefOf, isBlank, optionsOf, step, wordCount } from "./options";

const thread = (id: string): ApiThread => ({
  id,
  owner: "dev@example.com",
  title: "t",
  target: { agentId: "coder" },
  state: "done",
  createdAt: "2026-10-02T10:00:00Z",
  updatedAt: "2026-10-02T10:00:00Z",
  lastSeq: 9,
});
const snippet = { text: "x", cutStart: false, cutEnd: false, matches: [] };

describe("hits", () => {
  const result: ApiThreadHit = {
    thread: thread("0198a000-0000-7000-8000-000000000001"),
    rank: 2,
    messageHits: 2,
    hits: [
      { field: "title", snippet },
      { field: "message", seq: 7, role: "agent", at: "2026-10-02T10:00:00Z", snippet },
    ],
  };

  it("opens a message at its seq and a title at the thread", () => {
    const [title, message] = result.hits;
    expect(hrefOf(result.thread, title as never)).toBe(
      "/threads/0198a000-0000-7000-8000-000000000001",
    );
    expect(hrefOf(result.thread, message as never)).toBe(
      "/threads/0198a000-0000-7000-8000-000000000001#m-7",
    );
  });

  it("lists every hit once, in order, with ids the listbox can name", () => {
    const options = optionsOf([result, { ...result, thread: thread("other") }], "p");
    expect(options.map((o) => o.id)).toEqual(["p-0-0", "p-0-1", "p-1-0", "p-1-1"]);
  });
});

describe("step", () => {
  it("starts at the first on Down and the last on Up, and wraps", () => {
    expect(step(-1, 1, 3)).toBe(0);
    expect(step(-1, -1, 3)).toBe(2);
    expect(step(2, 1, 3)).toBe(0);
    expect(step(0, -1, 3)).toBe(2);
    expect(step(1, 1, 3)).toBe(2);
  });
  it("has nowhere to go with no hits", () => {
    expect(step(-1, 1, 0)).toBe(-1);
  });
});

describe("the query", () => {
  it("is blank by the contract's four characters and no others", () => {
    expect(isBlank("")).toBe(true);
    expect(isBlank(" \t\r\n ")).toBe(true);
    expect(isBlank(" ")).toBe(false);
    expect(isBlank(" a ")).toBe(false);
  });

  it("is clipped in Unicode scalar values, never through a surrogate pair", () => {
    expect(clip("abcdef", 3)).toBe("abc");
    expect(clip("😀😀😀", 2)).toBe("😀😀");
    expect(clip("short", 200)).toBe("short");
  });

  it("counts its words as the server does", () => {
    expect(wordCount("fix  the, login-page!")).toBe(4);
    expect(wordCount("!!")).toBe(0);
  });
});
