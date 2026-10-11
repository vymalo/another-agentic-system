import { describe, expect, it } from "vitest";
import { isBrandMark } from "@/components/brand/brand-marks";
import { GitHubMark } from "@/components/brand/github-mark";
import { OpenCodeMark } from "@/components/brand/opencode-mark";
import { parseStep, STEP_ICONS } from "@/features/chat/lib/agui/vymalo";
import {
  iconOf,
  KIND_ICON,
  knownServer,
  STEP_ICON,
  STEP_ICON_TITLE,
  stepGlyph,
} from "./step-icons";

/*
 * Which glyph a step is drawn with: the steps/v1 vocabulary on lucide icons, OpenCode's own mark for
 * `opencode`, and the mark or glyph of a server the page knows by the `server__tool` of its label
 * (ADR 0049, amended 2026-10-11).
 */

const tool = (label: string, icon?: (typeof STEP_ICONS)[number]) => ({
  kind: "tool" as const,
  label,
  ...(icon ? { icon } : {}),
});

describe("the steps/v1 vocabulary", () => {
  it("has a glyph for every id", () => {
    for (const id of STEP_ICONS) expect(STEP_ICON[id], id).toBeDefined();
  });

  it("is lucide's, but for OpenCode, which is drawn with its own mark", () => {
    const brands = STEP_ICONS.filter((id) => isBrandMark(STEP_ICON[id]));
    expect(brands).toEqual(["opencode"]);
    expect(STEP_ICON.opencode).toBe(OpenCodeMark);
    expect(STEP_ICON_TITLE.opencode).toBe("OpenCode");
  });

  it("drops an id it does not know, and the step falls back to the glyph of its kind", () => {
    const step = parseStep({ id: "a", label: "x", state: "completed", icon: "rocket" });
    expect(step?.icon).toBeUndefined();
    expect(iconOf({ kind: "tool", ...(step?.icon ? { icon: step.icon } : {}) })).toBe(
      KIND_ICON.tool,
    );
    expect(iconOf({ kind: "command" })).toBe(KIND_ICON.command);
  });
});

describe("stepGlyph", () => {
  it("draws the icon a step names", () => {
    expect(stepGlyph(tool("npm test", "test"))).toMatchObject({ icon: STEP_ICON.test });
    expect(stepGlyph(tool("OpenCode", "opencode"))).toEqual({
      icon: OpenCodeMark,
      title: "OpenCode",
    });
  });

  it("draws the GitHub mark for a GitHub tool, whatever icon the agent gave it", () => {
    for (const icon of [undefined, "git", "search", "tool", "read"] as const) {
      expect(stepGlyph(tool("github__list_commits", icon))).toEqual({
        icon: GitHubMark,
        title: "GitHub",
      });
    }
  });

  it("knows a server by the server__tool of its label, for a tool and for nothing else", () => {
    expect(knownServer("github__get_file_contents")?.name).toBe("GitHub");
    expect(knownServer("browser__browser_navigate")?.name).toBe("Browser");
    expect(knownServer("obscura__navigate")?.name).toBe("Browser");
    expect(knownServer("websearch__web_search")?.name).toBe("Web search");
    expect(knownServer("context7__resolve-library-id")?.name).toBe("Context7");
    for (const label of [
      "foo__bar",
      "github",
      "github_list",
      "run_checks",
      "GitHub · list commits",
    ]) {
      expect(knownServer(label), label).toBeUndefined();
    }
    expect(stepGlyph({ kind: "command", label: "github__list_commits" })).toEqual({
      icon: KIND_ICON.command,
      title: undefined,
    });
  });

  it("gives a server with no mark of its own a glyph, which gives way to an icon the agent chose", () => {
    const browser = knownServer("browser__browser_navigate");
    expect(stepGlyph(tool("browser__browser_navigate"))).toEqual({
      icon: browser?.icon,
      title: "Browser",
    });
    expect(stepGlyph(tool("browser__browser_navigate", "tool"))).toEqual({
      icon: browser?.icon,
      title: "Browser",
    });
    expect(stepGlyph(tool("browser__browser_navigate", "web"))).toMatchObject({
      icon: STEP_ICON.web,
    });
    expect(isBrandMark(browser?.icon ?? (() => null))).toBe(false);
  });

  it("leaves a server it does not know on the step's icon, then its kind's", () => {
    expect(stepGlyph(tool("foo__bar", "read"))).toEqual({ icon: STEP_ICON.read, title: undefined });
    expect(stepGlyph(tool("foo__bar"))).toEqual({ icon: KIND_ICON.tool, title: undefined });
    expect(stepGlyph({ kind: "message", label: "Done" })).toEqual({
      icon: KIND_ICON.message,
      title: undefined,
    });
  });
});
