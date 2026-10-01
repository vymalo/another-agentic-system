/*
 * The right-hand panel's rules, as pure functions: where it lives (docked beside the chat, or a
 * sheet), how wide it may be, and what the browser remembers of it. `PANEL_SCRIPT` repeats
 * `resolveOpen` for the `<head>`, where it must run before React: panel-state.test.ts runs both.
 */

/** localStorage keys, in the pattern of the sidebar's (`chat.sidebar`). */
export const PANEL_KEY = "chat.panel";
export const PANEL_WIDTH_KEY = "chat.panel.width";
export const PANEL_TAB_KEY = "chat.panel.tab";

/** The id the toggle's `aria-controls` points to: the docked panel's `<aside>` or the sheet. */
export const PANEL_ID = "thread-panel";

export const PANEL_TABS = ["activity", "sources"] as const;
export type PanelTab = (typeof PANEL_TABS)[number];

export const PANEL_MIN_WIDTH = 300;
export const PANEL_MAX_WIDTH = 560;
export const PANEL_DEFAULT_WIDTH = 360;

/** The sidebar's width (`w-68`): the layout counts on it being open, so the panel never jumps. */
const SIDEBAR_WIDTH = 272;
/** The reading column keeps at least this much beside the sidebar and the panel. */
const COLUMN_MIN_WIDTH = 560;
/** Below this the panel is a sheet from the bottom, as the thread list is a sheet on a phone. */
export const SHEET_BOTTOM_BELOW = 768;
/** From this width the panel docks: the sidebar, 560 px of chat and the narrowest panel fit. */
export const PANEL_DOCK_FROM = SIDEBAR_WIDTH + COLUMN_MIN_WIDTH + PANEL_MIN_WIDTH;
/** When the person never chose, the panel is open from this width (and closed below). */
export const PANEL_OPEN_FROM = 1280;

export type PanelLayout = "docked" | "sheet-right" | "sheet-bottom";

/** Docked beside the chat when it fits, else a sheet from the right, else one from the bottom. */
export function panelLayout(viewport: number): PanelLayout {
  if (viewport >= PANEL_DOCK_FROM) return "docked";
  return viewport >= SHEET_BOTTOM_BELOW ? "sheet-right" : "sheet-bottom";
}

/** The widest the docked panel may be: 560 px, 45 % of the window, and the chat keeps 560 px. */
export function maxPanelWidth(viewport: number): number {
  return Math.max(
    PANEL_MIN_WIDTH,
    Math.min(
      PANEL_MAX_WIDTH,
      Math.floor(viewport * 0.45),
      viewport - SIDEBAR_WIDTH - COLUMN_MIN_WIDTH,
    ),
  );
}

export function clampWidth(width: number, viewport: number): number {
  return Math.min(Math.max(Math.round(width), PANEL_MIN_WIDTH), maxPanelWidth(viewport));
}

/** A width the browser remembered: an integer inside the limits, else nothing. */
export function parseWidth(raw: string | null): number | null {
  if (raw === null || !/^\d{1,4}$/.test(raw)) return null;
  const width = Number(raw);
  return width >= PANEL_MIN_WIDTH && width <= PANEL_MAX_WIDTH ? width : null;
}

export function parseTab(raw: string | null): PanelTab | null {
  return (PANEL_TABS as readonly string[]).includes(raw ?? "") ? (raw as PanelTab) : null;
}

/** What the browser remembered says; with nothing remembered, open on a wide window only. */
export function resolveOpen(stored: string | null, viewport: number): boolean {
  if (stored === "open") return true;
  if (stored === "closed") return false;
  return viewport >= PANEL_OPEN_FROM;
}

/**
 * Runs in `<head>` before the first paint, like `SIDEBAR_SCRIPT`: an open panel is marked on
 * `<html>` (`data-panel="open"`) and its remembered width is put in `--panel-width`, and the
 * stylesheet docks it by that on a wide window until React's state, which starts closed for the
 * server's render, catches up. Storage may be blocked: then the default applies.
 */
export const PANEL_SCRIPT = `(function(){var d=document.documentElement,o=innerWidth>=${PANEL_OPEN_FROM};try{var s=localStorage.getItem(${JSON.stringify(PANEL_KEY)});if(s==="open")o=true;else if(s==="closed")o=false;var w=localStorage.getItem(${JSON.stringify(PANEL_WIDTH_KEY)});if(/^\\d{1,4}$/.test(w)&&w>=${PANEL_MIN_WIDTH}&&w<=${PANEL_MAX_WIDTH})d.style.setProperty("--panel-width",w+"px")}catch(e){}if(o)d.dataset.panel="open"})()`;
