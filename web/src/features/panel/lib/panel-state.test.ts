import { describe, expect, it } from "vitest";
import {
  clampWidth,
  maxPanelWidth,
  PANEL_DOCK_FROM,
  PANEL_KEY,
  PANEL_MAX_WIDTH,
  PANEL_MIN_WIDTH,
  PANEL_OPEN_FROM,
  PANEL_SCRIPT,
  PANEL_WIDTH_KEY,
  panelLayout,
  parseTab,
  parseWidth,
  resolveOpen,
} from "./panel-state";

describe("panelLayout", () => {
  it("docks the panel when the sidebar, 560 px of chat and the narrowest panel fit", () => {
    expect(PANEL_DOCK_FROM).toBe(272 + 560 + 300);
    expect(panelLayout(1440)).toBe("docked");
    expect(panelLayout(1280)).toBe("docked");
    expect(panelLayout(PANEL_DOCK_FROM)).toBe("docked");
  });

  it("is a sheet from the right below that and a sheet from the bottom on a phone", () => {
    expect(panelLayout(PANEL_DOCK_FROM - 1)).toBe("sheet-right");
    expect(panelLayout(1024)).toBe("sheet-right");
    expect(panelLayout(768)).toBe("sheet-right");
    expect(panelLayout(767)).toBe("sheet-bottom");
    expect(panelLayout(390)).toBe("sheet-bottom");
  });
});

describe("the width", () => {
  it("never leaves the chat less than 560 px beside the sidebar, nor more than 45 % or 560 px", () => {
    expect(maxPanelWidth(PANEL_DOCK_FROM)).toBe(PANEL_MIN_WIDTH);
    expect(maxPanelWidth(1280)).toBe(448); // 1280 - 272 - 560
    expect(maxPanelWidth(1440)).toBe(560);
    expect(maxPanelWidth(2560)).toBe(PANEL_MAX_WIDTH);
    // the 45 % rule cannot go under the minimum
    expect(maxPanelWidth(500)).toBe(PANEL_MIN_WIDTH);
  });

  it("clamps a wanted width into the limits of the window", () => {
    expect(clampWidth(100, 1440)).toBe(300);
    expect(clampWidth(360, 1440)).toBe(360);
    expect(clampWidth(900, 1440)).toBe(560);
    expect(clampWidth(500, 1280)).toBe(448);
    expect(clampWidth(360.6, 1440)).toBe(361);
  });

  it("reads a remembered width only when it is an integer inside the limits", () => {
    expect(parseWidth("360")).toBe(360);
    expect(parseWidth("300")).toBe(300);
    expect(parseWidth("560")).toBe(560);
    for (const bad of [
      null,
      "",
      "299",
      "561",
      "36.5",
      "-360",
      "abc",
      "360px",
      "1e3",
      " 360",
      "99999",
    ]) {
      expect(parseWidth(bad), String(bad)).toBeNull();
    }
  });
});

describe("what is remembered", () => {
  it("reads a tab it knows, nothing else", () => {
    expect(parseTab("activity")).toBe("activity");
    expect(parseTab("sources")).toBe("sources");
    expect(parseTab("steps")).toBeNull();
    expect(parseTab(null)).toBeNull();
  });

  it("opens by the remembered choice, and with none on a wide window only", () => {
    expect(resolveOpen("open", 400)).toBe(true);
    expect(resolveOpen("closed", 2000)).toBe(false);
    expect(resolveOpen(null, PANEL_OPEN_FROM)).toBe(true);
    expect(resolveOpen(null, PANEL_OPEN_FROM - 1)).toBe(false);
    expect(resolveOpen("garbage", 2000)).toBe(true);
    expect(resolveOpen("garbage", 1000)).toBe(false);
  });
});

/** The head script, run against a fake page: what it leaves on `<html>`. */
function runScript(opts: { innerWidth: number; storage?: Record<string, string> | "blocked" }): {
  panel: string | undefined;
  width: string | undefined;
} {
  const style = new Map<string, string>();
  const root = {
    dataset: {} as Record<string, string>,
    style: { setProperty: (k: string, v: string) => void style.set(k, v) },
  };
  const localStorage = {
    getItem: (k: string) => {
      if (opts.storage === "blocked") throw new Error("SecurityError");
      return opts.storage?.[k] ?? null;
    },
  };
  new Function("document", "innerWidth", "localStorage", PANEL_SCRIPT)(
    { documentElement: root },
    opts.innerWidth,
    localStorage,
  );
  return { panel: root.dataset.panel, width: style.get("--panel-width") };
}

describe("PANEL_SCRIPT", () => {
  it("marks the panel open on the same rule as resolveOpen", () => {
    for (const stored of [undefined, "open", "closed", "garbage"]) {
      for (const innerWidth of [390, 1024, 1279, 1280, 1920]) {
        const { panel } = runScript({
          innerWidth,
          storage: stored === undefined ? {} : { [PANEL_KEY]: stored },
        });
        expect(panel === "open", `${stored} at ${innerWidth}`).toBe(
          resolveOpen(stored ?? null, innerWidth),
        );
      }
    }
  });

  it("puts a remembered width in --panel-width, and only a valid one", () => {
    expect(runScript({ innerWidth: 1440, storage: { [PANEL_WIDTH_KEY]: "420" } }).width).toBe(
      "420px",
    );
    for (const bad of ["299", "561", "abc", "36.5", ""]) {
      expect(
        runScript({ innerWidth: 1440, storage: { [PANEL_WIDTH_KEY]: bad } }).width,
      ).toBeUndefined();
    }
  });

  it("falls back to the default when storage is blocked, and does not throw", () => {
    expect(runScript({ innerWidth: 1440, storage: "blocked" }).panel).toBe("open");
    expect(runScript({ innerWidth: 1000, storage: "blocked" }).panel).toBeUndefined();
  });
});
