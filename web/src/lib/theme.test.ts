// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  applyTheme,
  currentTheme,
  getChoice,
  parseChoice,
  resolveTheme,
  setChoice,
  subscribeTheme,
  THEME_COLORS,
  THEME_KEY,
  THEME_SCRIPT,
} from "./theme";

/** A `matchMedia` that says whether the device is dark, and can change its mind. */
function device(dark: boolean) {
  const listeners = new Set<() => void>();
  const state = { dark };
  vi.stubGlobal("matchMedia", (query: string) => ({
    media: query,
    get matches() {
      return query.includes("dark") ? state.dark : !state.dark;
    },
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
  }));
  return {
    flip(to: boolean) {
      state.dark = to;
      for (const fn of [...listeners]) fn();
    },
  };
}

const root = () => document.documentElement;

beforeEach(() => {
  localStorage.clear();
  root().removeAttribute("data-theme");
  document.head.innerHTML =
    '<meta name="theme-color" content="#ffffff" media="(prefers-color-scheme: light)">' +
    '<meta name="theme-color" content="#141614" media="(prefers-color-scheme: dark)">';
});
afterEach(() => {
  vi.restoreAllMocks();
  setChoice("system");
  vi.unstubAllGlobals();
});

describe("the choice", () => {
  it("is one of three words, and anything else is System", () => {
    expect(parseChoice("light")).toBe("light");
    expect(parseChoice("dark")).toBe("dark");
    expect(parseChoice("system")).toBe("system");
    for (const bad of [null, "", "Dark", "auto", "1"]) expect(parseChoice(bad)).toBe("system");
  });

  it("resolves to the device's scheme only while it is System", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
    expect(resolveTheme("light", true)).toBe("light");
  });
});

describe("setChoice and applyTheme", () => {
  it("marks the root with the resolved theme and remembers the choice on this device", () => {
    device(false);
    setChoice("dark");
    expect(root().dataset.theme).toBe("dark");
    expect(currentTheme()).toBe("dark");
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    expect(getChoice()).toBe("dark");
  });

  it("follows the device live while it is System, and stops following once a theme is forced", () => {
    const d = device(false);
    const stop = subscribeTheme(() => {});
    setChoice("system");
    expect(root().dataset.theme).toBe("light");
    d.flip(true);
    expect(root().dataset.theme).toBe("dark");
    setChoice("light");
    d.flip(false);
    d.flip(true);
    expect(root().dataset.theme).toBe("light");
    stop();
  });

  it("tells its subscribers when the theme or the choice changes", () => {
    const d = device(false);
    const seen = vi.fn();
    const stop = subscribeTheme(seen);
    setChoice("dark");
    expect(seen).toHaveBeenCalledTimes(1);
    setChoice("system");
    seen.mockClear();
    d.flip(true);
    expect(seen).toHaveBeenCalledTimes(1);
    stop();
  });

  it("stores the word System when System is chosen, never the theme it resolves to", () => {
    device(true);
    setChoice("system");
    expect(localStorage.getItem(THEME_KEY)).toBe("system");
    expect(root().dataset.theme).toBe("dark");
    setChoice("light");
    setChoice("system");
    expect(localStorage.getItem(THEME_KEY)).toBe("system");
  });

  it("follows another tab's choice, and re-reads when another tab clears storage (a storage event with no key)", () => {
    device(false);
    const seen = vi.fn();
    const stop = subscribeTheme(seen);
    setChoice("dark");
    expect(root().dataset.theme).toBe("dark");
    // another tab chose Light
    localStorage.setItem(THEME_KEY, "light");
    window.dispatchEvent(new StorageEvent("storage", { key: THEME_KEY, newValue: "light" }));
    expect(getChoice()).toBe("light");
    expect(root().dataset.theme).toBe("light");
    // another tab chose Dark, then cleared everything: `localStorage.clear()` reports key null
    setChoice("dark");
    localStorage.clear();
    window.dispatchEvent(new StorageEvent("storage", { key: null }));
    expect(getChoice()).toBe("system");
    expect(root().dataset.theme).toBe("light");
    // a key of something else is no business of the theme's
    setChoice("dark");
    window.dispatchEvent(new StorageEvent("storage", { key: "chat.sidebar", newValue: "closed" }));
    expect(getChoice()).toBe("dark");
    stop();
  });

  it("keeps working for the session when storage refuses", () => {
    device(false);
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(() => setChoice("dark")).not.toThrow();
    expect(root().dataset.theme).toBe("dark");
    expect(getChoice()).toBe("dark");
  });

  it("sets the address bar's colour to the chosen theme's, and gives the device its two back on System", () => {
    device(false);
    const metas = () =>
      [...document.querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]')].map(
        (m) => m.content,
      );
    applyTheme("dark");
    expect(metas()).toEqual([THEME_COLORS.dark, THEME_COLORS.dark]);
    applyTheme("system");
    expect(metas()).toEqual([THEME_COLORS.light, THEME_COLORS.dark]);
  });
});

/** Runs the head script against a fake document: what it leaves on the root, before any React. */
function runScript(opts: { stored?: string; dark: boolean; storageThrows?: boolean }) {
  const dataset: Record<string, string> = {};
  new Function("document", "matchMedia", "localStorage", THEME_SCRIPT)(
    { documentElement: { dataset } },
    (q: string) => ({ matches: q.includes("dark") ? opts.dark : !opts.dark }),
    {
      getItem() {
        if (opts.storageThrows) throw new Error("blocked");
        return opts.stored ?? null;
      },
    },
  );
  return dataset.theme;
}

describe("THEME_SCRIPT (the head script, before the first paint)", () => {
  it("resolves on the same rule as resolveTheme, for every stored value and both devices", () => {
    for (const stored of [undefined, "system", "light", "dark", "garbage"]) {
      for (const dark of [true, false]) {
        expect(runScript({ stored, dark }), `${stored} on ${dark ? "dark" : "light"}`).toBe(
          resolveTheme(parseChoice(stored ?? null), dark),
        );
      }
    }
  });

  it("follows the device when storage is refused", () => {
    expect(runScript({ dark: true, storageThrows: true })).toBe("dark");
    expect(runScript({ dark: false, storageThrows: true })).toBe("light");
  });

  it("is a constant with nothing of the input in it", () => {
    expect(THEME_SCRIPT).toContain(JSON.stringify(THEME_KEY));
    expect(THEME_SCRIPT).not.toMatch(/eval|innerHTML|document\.write|import/);
  });
});
