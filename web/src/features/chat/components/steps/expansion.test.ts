import { describe, expect, it } from "vitest";
import { FIRST_SHOWN, NO_EXPANSION, nodeKey, shownOf, withPathOpen, withShown } from "./expansion";

const node = (id: string, kind: string, children: string[] = []) => ({
  id,
  kind,
  children: children.map((c) => ({ id: c })),
});
const k = (id: string) => nodeKey("t1", id);

describe("opening the way to a step", () => {
  const kids = Array.from({ length: 12 }, (_, i) => `k${i}`);

  it("lists each ancestor far enough back to include the next one, and opens the step", () => {
    const path = [node("turn", "agent", ["oc"]), node("oc", "subagent", kids), node("k2", "tool")];
    const next = withPathOpen(NO_EXPANSION, "t1", path);
    // k2 is the 3rd of 12: ten from the end is the oldest the level must list
    expect(shownOf(next, k("oc"))).toBe(10);
    expect(shownOf(next, k("k2"))).toBe(1);
    // the turn's own root is not a row
    expect(next.nodes.has(k("turn"))).toBe(false);
  });

  it("opens at least the latest few of a step that has children, and a leaf as one", () => {
    const next = withPathOpen(NO_EXPANSION, "t1", [
      node("turn", "agent"),
      node("oc", "subagent", ["a", "b"]),
    ]);
    expect(shownOf(next, k("oc"))).toBe(FIRST_SHOWN);
    expect(shownOf(withPathOpen(NO_EXPANSION, "t1", [node("leaf", "tool")]), k("leaf"))).toBe(1);
  });

  it("never closes or narrows what is open", () => {
    const open = withShown(NO_EXPANSION, k("oc"), 33);
    const next = withPathOpen(open, "t1", [node("oc", "subagent", kids), node("k11", "tool")]);
    expect(shownOf(next, k("oc"))).toBe(33);
    expect(shownOf(next, k("k11"))).toBe(1);
  });
});
