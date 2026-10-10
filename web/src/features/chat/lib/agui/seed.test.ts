import type { ThreadMessage } from "@assistant-ui/react";
import { describe, expect, it } from "vitest";
import { asRepository, joinMessages } from "./seed";

const message = (id: string, text = id): ThreadMessage =>
  ({
    id,
    role: "user",
    createdAt: new Date(0),
    content: [{ type: "text", text }],
    attachments: [],
    metadata: { custom: {} },
  }) as unknown as ThreadMessage;

const ids = (list: readonly ThreadMessage[]) => list.map((m) => m.id);

describe("joining older messages in front of the current ones", () => {
  it("puts the older ones first and keeps the order of each", () => {
    expect(ids(joinMessages([message("a"), message("b")], [message("c"), message("d")]))).toEqual([
      "a",
      "b",
      "c",
      "d",
    ]);
    expect(ids(joinMessages([], [message("c")]))).toEqual(["c"]);
    expect(ids(joinMessages([message("a")], []))).toEqual(["a"]);
  });

  it("keeps the newest copy of an id both hold, in the newer place", () => {
    const joined = joinMessages(
      [message("a"), message("s", "surface, early"), message("b")],
      [message("s", "surface, later"), message("c")],
    );
    expect(ids(joined)).toEqual(["a", "b", "s", "c"]);
    const kept = joined.find((m) => m.id === "s");
    expect(kept?.content[0]).toEqual({ type: "text", text: "surface, later" });
  });

  it("never holds an id twice and keeps every id, whatever the two lists share (random lists)", () => {
    let seed = 99;
    const rnd = (n: number) => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed % n;
    };
    for (let round = 0; round < 200; round++) {
      const pick = () =>
        Array.from({ length: rnd(8) }, () => `m${rnd(10)}`).filter(
          (id, i, all) => all.indexOf(id) === i,
        );
      const older = pick().map((id) => message(id, `older ${id}`));
      const current = pick().map((id) => message(id, `current ${id}`));
      const joined = joinMessages(older, current);
      const got = ids(joined);
      expect(new Set(got).size).toBe(got.length);
      expect(new Set(got)).toEqual(new Set([...ids(older), ...ids(current)]));
      // the current ones are all there, last, in their order
      expect(got.slice(got.length - current.length)).toEqual(ids(current));
      // and a shared id is the current copy
      for (const m of joined) {
        if (ids(current).includes(m.id)) {
          expect(m.content[0]).toEqual({ type: "text", text: `current ${m.id}` });
        }
      }
    }
  });
});

describe("a transcript as an import takes it", () => {
  it("is a chain: each message's parent is the one before, and the head is the last", () => {
    const repo = asRepository([message("a"), message("b"), message("c")]);
    expect(repo.headId).toBe("c");
    expect(repo.messages.map((m) => [m.message.id, m.parentId])).toEqual([
      ["a", null],
      ["b", "a"],
      ["c", "b"],
    ]);
    expect(asRepository([])).toEqual({ headId: null, messages: [] });
  });
});
