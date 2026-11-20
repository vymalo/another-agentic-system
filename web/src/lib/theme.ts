/*
 * The theme of the page: System (follows the device), Light or Dark, chosen by the person and kept on this device
 * (localStorage; ADR 0011, Amendment 2026-11-14; web/DESIGN.md "Theme"). It is a display preference of this client and
 * of nothing else: no API field carries it, and another client keeps its own.
 *
 * The page's theme is ONE attribute on `<html>`, `data-theme="light"` or `data-theme="dark"`, always the resolved one
 * (never "system"). The stylesheet, the Tailwind `dark:` variant, the highlighter's colours and the diagrams all read
 * it, never `prefers-color-scheme`: a component that asked the device would draw a dark diagram on a light page.
 * `THEME_SCRIPT` sets it in `<head>` before the first paint (the layout allows it by its hash, `lib/csp.ts`);
 * the functions below keep it right afterwards.
 */

export type ThemeChoice = "system" | "light" | "dark";
export type Theme = "light" | "dark";

/** The localStorage key, in the pattern of the sidebar's (`chat.sidebar`). */
export const THEME_KEY = "chat.theme";

export const THEME_CHOICES: readonly ThemeChoice[] = ["system", "light", "dark"];

/** The address bar's colour for each theme: the page's `--background` (`globals.css`). */
export const THEME_COLORS: Record<Theme, string> = { light: "#ffffff", dark: "#141614" };

const DARK_QUERY = "(prefers-color-scheme: dark)";

/** The stored word, or System for anything that is not one of the three. */
export function parseChoice(raw: string | null | undefined): ThemeChoice {
  return raw === "light" || raw === "dark" ? raw : "system";
}

/** The theme a choice gives on a device that is `systemDark`. */
export function resolveTheme(choice: ThemeChoice, systemDark: boolean): Theme {
  if (choice === "system") return systemDark ? "dark" : "light";
  return choice;
}

/**
 * Runs in `<head>` before the first paint, like `SIDEBAR_SCRIPT`: puts the resolved theme on `<html>`, so the stylesheet's
 * first paint is the right one. Storage may be refused: then the device decides. The same rule as `resolveTheme`
 * (theme.test.ts runs both).
 */
export const THEME_SCRIPT = `(function(){var s=null,d=false;try{s=localStorage.getItem(${JSON.stringify(THEME_KEY)})}catch(e){}try{d=matchMedia(${JSON.stringify(DARK_QUERY)}).matches}catch(e){}document.documentElement.dataset.theme=s==="light"||s==="dark"?s:d?"dark":"light"})()`;

// What this page remembers when storage refuses: the choice lasts for the session.
let choice: ThemeChoice | null = null;
const listeners = new Set<() => void>();
let detach: (() => void) | undefined;

function media(): MediaQueryList | undefined {
  return typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia(DARK_QUERY)
    : undefined;
}

const systemDark = () => media()?.matches ?? false;

/** The person's choice on this device: System until they chose. */
export function getChoice(): ThemeChoice {
  if (choice === null) {
    try {
      choice = parseChoice(localStorage.getItem(THEME_KEY));
    } catch {
      choice = "system";
    }
  }
  return choice;
}

/** The theme the page shows now (what `<html data-theme>` says). */
export function currentTheme(): Theme {
  if (typeof document === "undefined") return "light";
  return document.documentElement.dataset.theme === "dark" ? "dark" : "light";
}

/** Puts the theme of `value` on the page: `<html data-theme>`, and the address bar's colour. */
export function applyTheme(value: ThemeChoice): Theme {
  const theme = resolveTheme(value, systemDark());
  if (typeof document === "undefined") return theme;
  // never a write of what is already there: the head script has set it, and a second write is a style recalculation
  if (document.documentElement.dataset.theme !== theme)
    document.documentElement.dataset.theme = theme;
  // the layout writes one `theme-color` per scheme, picked by the device; a chosen theme overrides both, System gives
  // each its own colour back
  for (const meta of document.querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]')) {
    const query = meta.getAttribute("media");
    const own: Theme = query?.includes("dark") ? "dark" : "light";
    meta.content = THEME_COLORS[value === "system" && query ? own : theme];
  }
  return theme;
}

const notify = () => {
  for (const fn of [...listeners]) fn();
};

/** Chooses a theme: shown at once, kept on this device (when the browser lets it) and told to the subscribers. */
export function setChoice(value: ThemeChoice): void {
  choice = value;
  try {
    localStorage.setItem(THEME_KEY, value);
  } catch {
    // storage refused: the choice lasts for this session
  }
  applyTheme(value);
  notify();
}

/**
 * Follows what changes the theme without a click: the device while the choice is System, and another tab's choice.
 * Returns the way to stop; the first subscriber starts the listening, the last one ends it.
 */
export function subscribeTheme(listener: () => void): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    const query = media();
    const onDevice = () => {
      if (getChoice() !== "system") return;
      applyTheme("system");
      notify();
    };
    const onStorage = (event: StorageEvent) => {
      // a key of something else is not ours; no key at all is `localStorage.clear()` in another tab: read it again
      if (event.key !== null && event.key !== THEME_KEY) return;
      if (event.key === null) {
        try {
          choice = parseChoice(localStorage.getItem(THEME_KEY));
        } catch {
          choice = "system";
        }
      } else {
        choice = parseChoice(event.newValue);
      }
      applyTheme(choice);
      notify();
    };
    query?.addEventListener("change", onDevice);
    window.addEventListener("storage", onStorage);
    detach = () => {
      query?.removeEventListener("change", onDevice);
      window.removeEventListener("storage", onStorage);
    };
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      detach?.();
      detach = undefined;
    }
  };
}
