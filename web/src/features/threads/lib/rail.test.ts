import { describe, expect, it } from "vitest";
import type { ApiThread } from "@/lib/api/types";
import { actionsFor, arrangementFor, blocksOf, lastRoot, type RowAction, sectionsOf } from "./rail";

const thread = (id: string, extra: Partial<ApiThread> = {}): ApiThread => ({
  id,
  owner: "dev@example.com",
  title: id,
  target: { agentId: "coder" },
  state: "done",
  createdAt: "2026-10-02T10:00:00Z",
  updatedAt: "2026-10-02T10:00:00Z",
  lastSeq: 1,
  ...extra,
});

const kinds = (list: readonly { kind: RowAction }[]) => list.map((a) => a.kind);

describe("blocksOf: the rows of order=rail as blocks", () => {
  it("puts each nested row under the top-level row before it, in the order the API gave", () => {
    const rows = [
      thread("a"),
      thread("a2", { nestedUnder: "a" }),
      thread("a1", { nestedUnder: "a" }),
      thread("b"),
    ];
    expect(blocksOf(rows).map((b) => [b.root.id, b.children.map((c) => c.id)])).toEqual([
      ["a", ["a2", "a1"]],
      ["b", []],
    ]);
  });

  it("stands a row alone when the thread it is nested under is not on the page (an archived child)", () => {
    const rows = [thread("kid", { nestedUnder: "gone", archived: true }), thread("b")];
    expect(blocksOf(rows).map((b) => b.root.id)).toEqual(["kid", "b"]);
  });

  it("has nothing for no rows", () => {
    expect(blocksOf([])).toEqual([]);
  });
});

describe("lastRoot: the cursor of the next page", () => {
  it("is the last top-level row, never one of its children", () => {
    const rows = [thread("a"), thread("b"), thread("b1", { nestedUnder: "b" })];
    expect(lastRoot(rows)).toBe("b");
  });

  it("is undefined for an empty page", () => {
    expect(lastRoot([])).toBeUndefined();
  });
});

describe("sectionsOf: Pinned and Chats", () => {
  it("splits the blocks on their root's pin and keeps each section's order", () => {
    const rows = [
      thread("p1", { pinned: true }),
      thread("p1k", { pinned: true, nestedUnder: "p1" }),
      thread("p2", { pinned: true }),
      thread("c1"),
      thread("c2"),
    ];
    const { pinned, chats } = sectionsOf(blocksOf(rows));
    expect(pinned.map((b) => b.root.id)).toEqual(["p1", "p2"]);
    expect(chats.map((b) => b.root.id)).toEqual(["c1", "c2"]);
    // a child is shown in the section of its root: it carries no pin of its own
    expect(pinned[0]?.children.map((c) => c.id)).toEqual(["p1k"]);
  });
});

describe("actionsFor: what the row's menu offers (ADR 0042, decisions 4 and 6; ADR 0043)", () => {
  it("a top-level chat: Pin, Archive and Delete", () => {
    expect(kinds(actionsFor(thread("a"), { section: "chats", mayDelete: true }))).toEqual([
      "pin",
      "archive",
      "delete",
    ]);
  });

  it("a pinned chat: Unpin instead of Pin", () => {
    expect(
      kinds(actionsFor(thread("a", { pinned: true }), { section: "pinned", mayDelete: true })),
    ).toEqual(["unpin", "archive", "delete"]);
  });

  it("a child: Eject from parent and Archive, and no Pin", () => {
    const actions = actionsFor(thread("k", { nestedUnder: "a" }), {
      section: "chats",
      mayDelete: true,
    });
    expect(kinds(actions)).toEqual(["eject", "archive", "delete"]);
    expect(kinds(actions)).not.toContain("pin");
  });

  it("an archived chat: Unarchive and Delete, and no Pin", () => {
    expect(
      kinds(actionsFor(thread("a", { archived: true }), { section: "archived", mayDelete: true })),
    ).toEqual(["unarchive", "delete"]);
  });

  it("a child archived alone: Unarchive, Eject and Delete", () => {
    const row = thread("k", { nestedUnder: "a", archived: true });
    expect(kinds(actionsFor(row, { section: "archived", mayDelete: true }))).toEqual([
      "unarchive",
      "eject",
      "delete",
    ]);
  });

  it("a child that is in Archived only because its parent is: Eject and Delete (it has no archive of its own)", () => {
    const row = thread("k", { nestedUnder: "a" });
    expect(kinds(actionsFor(row, { section: "archived", mayDelete: true }))).toEqual([
      "eject",
      "delete",
    ]);
  });

  it("no Delete without the permission, on any kind of row", () => {
    const rows: [ApiThread, "pinned" | "chats" | "archived"][] = [
      [thread("a"), "chats"],
      [thread("p", { pinned: true }), "pinned"],
      [thread("k", { nestedUnder: "a" }), "chats"],
      [thread("z", { archived: true }), "archived"],
    ];
    for (const [row, section] of rows) {
      expect(kinds(actionsFor(row, { section, mayDelete: false }))).not.toContain("delete");
    }
  });

  it("every action has words", () => {
    const all = actionsFor(thread("k", { nestedUnder: "a", archived: true }), {
      section: "archived",
      mayDelete: true,
    });
    expect(all.map((a) => a.label)).toEqual(["Unarchive", "Eject from parent", "Delete"]);
  });
});

describe("arrangementFor: one arrangeThread body per action", () => {
  it("is the member the action names", () => {
    expect(arrangementFor("pin")).toEqual({ pinned: true });
    expect(arrangementFor("unpin")).toEqual({ pinned: false });
    expect(arrangementFor("archive")).toEqual({ archived: true });
    expect(arrangementFor("unarchive")).toEqual({ archived: false });
    expect(arrangementFor("eject")).toEqual({ nested: false });
  });

  it("has none for Delete, which is not an arrangement", () => {
    expect(arrangementFor("delete")).toBeUndefined();
  });
});
