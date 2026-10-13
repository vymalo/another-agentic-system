import type { ApiArrangement, ApiThread } from "@/lib/api/types";

/*
 * The person's own list (ADR 0042): `GET /api/threads?order=rail` gives each top-level thread
 * followed by the threads nested under it (one level deep, newest first), the pinned first. These
 * are the pure rules the sidebar draws from: the blocks, the sections, the cursor of the next page
 * and what a row's menu offers. The server owns the order; nothing here ranks anything.
 */

/** A top-level row of the list with the rows nested under it. */
export type Block = { root: ApiThread; children: ApiThread[] };

/**
 * Groups rows, in the order the API gave them, into blocks. A row nested under a thread that is
 * not on the page stands on its own: a child archived by itself is listed in Archived without its
 * parent, which is not archived.
 */
export function blocksOf(rows: readonly ApiThread[]): Block[] {
  const blocks: Block[] = [];
  const byRoot = new Map<string, Block>();
  for (const row of rows) {
    const under = row.nestedUnder === undefined ? undefined : byRoot.get(row.nestedUnder);
    if (under) {
      under.children.push(row);
      continue;
    }
    const own: Block = { root: row, children: [] };
    blocks.push(own);
    byRoot.set(row.id, own);
  }
  return blocks;
}

/** The last top-level row of a page: the `before` of the next one (`limit` counts these, so a block is never split). */
export const lastRoot = (rows: readonly ApiThread[]): string | undefined =>
  blocksOf(rows).at(-1)?.root.id;

/** The pinned blocks and the others, each in the order the server gave; a child goes with its root. */
export function sectionsOf(blocks: readonly Block[]): { pinned: Block[]; chats: Block[] } {
  return {
    pinned: blocks.filter((b) => b.root.pinned === true),
    chats: blocks.filter((b) => b.root.pinned !== true),
  };
}

export type Section = "pinned" | "chats" | "archived";

export type RowAction = "pin" | "unpin" | "archive" | "unarchive" | "eject" | "delete";

export type RowMenuItem = { kind: RowAction; label: string; hint?: string };

const LABELS: Record<RowAction, string> = {
  pin: "Pin",
  unpin: "Unpin",
  archive: "Archive",
  unarchive: "Unarchive",
  eject: "Eject from parent",
  delete: "Delete",
};

const item = (kind: RowAction): RowMenuItem => ({
  kind,
  label: LABELS[kind],
  ...(kind === "eject"
    ? {
        hint: "Take this chat out of its parent and list it on its own. A chat inside another cannot be pinned.",
      }
    : {}),
});

/**
 * What a row's menu offers (ADR 0042, decisions 4 and 6; ADR 0043, decision 6):
 *
 * - Pin and Unpin act on a top-level chat that is not archived; a child cannot be pinned (it is shown
 *   in its root's section), so it is offered "Eject from parent" instead, whose hint says why.
 * - Archive acts on the row itself (a child archives alone); Unarchive undoes an archive of its own.
 * - A child is in Archived either because it was archived by itself or because its parent was: only
 *   the first has an archive to undo, and Eject takes either out of the block.
 * - Delete is there only with `thread.delete`; the server refuses it otherwise, so this is for the eye.
 */
export function actionsFor(
  thread: ApiThread,
  ctx: { section: Section; mayDelete: boolean },
): RowMenuItem[] {
  const child = thread.nestedUnder !== undefined;
  const kinds: RowAction[] = [];
  if (ctx.section === "archived") {
    if (thread.archived === true) kinds.push("unarchive");
    if (child) kinds.push("eject");
  } else if (child) {
    kinds.push("eject", "archive");
  } else {
    kinds.push(thread.pinned === true ? "unpin" : "pin", "archive");
  }
  if (ctx.mayDelete) kinds.push("delete");
  return kinds.map(item);
}

/** The `arrangeThread` body of an action; Delete is not an arrangement. */
export function arrangementFor(action: RowAction): ApiArrangement | undefined {
  switch (action) {
    case "pin":
      return { pinned: true };
    case "unpin":
      return { pinned: false };
    case "archive":
      return { archived: true };
    case "unarchive":
      return { archived: false };
    case "eject":
      return { nested: false };
    case "delete":
      return undefined;
  }
}
