import { describe, expect, it } from "vitest";
import { ACTIVITY, activityPartName } from "@/features/chat/lib/agui/vymalo";
import { turnSource } from "./turn-source";

const surface = (types: string[]) => ({
  type: "data",
  name: activityPartName(ACTIVITY.surface),
  data: {
    a2ui_operations: [
      { createSurface: { surfaceId: "s" } },
      {
        updateComponents: {
          surfaceId: "s",
          components: types.map((component, i) => ({ id: `c${i}`, component })),
        },
      },
    ],
  },
});

describe("what Copy takes of a turn", () => {
  it("is the source text of the answer, working text left out, parts joined by a blank line", () => {
    const content = [
      { type: "text", text: "looking" },
      { type: "text", text: "# Answer\n\n**bold**" },
      { type: "text", text: "and more" },
    ];
    const roles = new Map<number, "answer" | "working">([
      [0, "working"],
      [1, "answer"],
      [2, "answer"],
    ]);
    expect(turnSource(content, roles)).toBe("# Answer\n\n**bold**\n\nand more");
  });

  it("copies the line of a turn that only drew an interface", () => {
    expect(turnSource([surface(["Column", "Choices"])], new Map())).toBe("[Interface: Choices]");
  });

  it("puts the interface line where the surface was", () => {
    const content = [
      { type: "text", text: "before" },
      surface(["Cards"]),
      { type: "text", text: "after" },
    ];
    const roles = new Map<number, "answer" | "working">([
      [0, "answer"],
      [2, "answer"],
    ]);
    expect(turnSource(content, roles)).toBe("before\n\n[Interface: Cards]\n\nafter");
  });

  it("is nothing for a turn with no words and no surface", () => {
    const content = [
      { type: "text", text: "  " },
      { type: "data", name: "vymalo.step", data: {} },
    ];
    expect(turnSource(content, new Map())).toBe("");
  });
});
