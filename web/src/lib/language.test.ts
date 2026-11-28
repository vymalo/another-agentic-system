// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isInterfaceTag } from "@/features/locale/lib/locale";
import {
  applyLanguage,
  DEFAULT_LANGUAGES,
  getLanguage,
  LANGUAGE_KEY,
  LANGUAGE_NAMES,
  LANGUAGE_SCRIPT,
  languageName,
  PAGE_LANGUAGE,
  parseLanguage,
  resetLanguage,
  setLanguage,
  subscribeLanguage,
} from "./language";

/*
 * The interface language a person picks (ADR 0085; web/README.md "The interface language"): kept on this
 * device under `chat.lang`, put on `<html lang>` by the head script before the first paint, and read by
 * the send path as it always was (`features/locale/lib/locale.ts`).
 */

const root = () => document.documentElement;

beforeEach(() => {
  localStorage.clear();
  resetLanguage();
  root().lang = PAGE_LANGUAGE;
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

/**
 * A tag the client sends, a tag it refuses, and the shapes in between. The rule is the tag's shape and not
 * whether it is a registered language (RFC 5646 well-formedness, as the orchestrator's `well_formed`): a
 * subtag of five to eight letters is well-formed, so `english` and `toString` pass and are sent.
 */
const ACCEPTED: readonly string[] = [
  ...DEFAULT_LANGUAGES,
  "en",
  "de-DE",
  "zh-Hant-TW",
  "sr-Latn-RS",
  "es-419",
  "nn-NO",
  "english",
  "toString",
];
const REFUSED: readonly (string | null | undefined)[] = [
  null,
  undefined,
  "",
  "e",
  "abcdefghi",
  "en_CM",
  "EN-gb-1901",
  "en-GB-1901",
  "en-GB-x-foo",
  "x-private",
  " en-GB",
  "en-GB ",
  "fr-FR;q=1",
  "en--GB",
];

describe("parseLanguage", () => {
  it("takes a language, an optional script and an optional region, and nothing else", () => {
    for (const tag of ACCEPTED) expect(parseLanguage(tag), String(tag)).toBe(tag);
    for (const tag of REFUSED) expect(parseLanguage(tag), String(tag)).toBeNull();
  });

  it("is the rule the messages already go out under: isInterfaceTag", () => {
    for (const tag of [...ACCEPTED, ...REFUSED.filter((t): t is string => typeof t === "string")])
      expect(parseLanguage(tag), tag).toBe(isInterfaceTag(tag) ? tag : null);
  });
});

describe("setLanguage and getLanguage", () => {
  it("puts the tag on <html> at once and remembers it on this device", () => {
    setLanguage("fr-FR");
    expect(root().lang).toBe("fr-FR");
    expect(localStorage.getItem(LANGUAGE_KEY)).toBe("fr-FR");
    expect(getLanguage()).toBe("fr-FR");
  });

  it("keeps the page's own tag when nothing was chosen", () => {
    expect(getLanguage()).toBeNull();
    expect(root().lang).toBe(PAGE_LANGUAGE);
  });

  it("writes nothing for a tag the client would not send", () => {
    setLanguage("en_CM");
    expect(localStorage.getItem(LANGUAGE_KEY)).toBeNull();
    expect(root().lang).toBe(PAGE_LANGUAGE);
    expect(getLanguage()).toBeNull();
  });

  it("follows another tab's choice, and goes back to the page's tag when another tab clears storage", () => {
    const seen = vi.fn();
    const stop = subscribeLanguage(seen);
    setLanguage("fr-FR");
    expect(seen).toHaveBeenCalledTimes(1);

    localStorage.setItem(LANGUAGE_KEY, "gsw-CH");
    window.dispatchEvent(new StorageEvent("storage", { key: LANGUAGE_KEY, newValue: "gsw-CH" }));
    expect(root().lang).toBe("gsw-CH");
    expect(getLanguage()).toBe("gsw-CH");

    // `localStorage.clear()` in another tab reports a storage event with no key
    localStorage.clear();
    window.dispatchEvent(new StorageEvent("storage", { key: null }));
    expect(root().lang).toBe(PAGE_LANGUAGE);
    expect(getLanguage()).toBeNull();

    // a key of something else is no business of the language's
    setLanguage("fur-IT");
    seen.mockClear();
    window.dispatchEvent(new StorageEvent("storage", { key: "chat.theme", newValue: "dark" }));
    expect(root().lang).toBe("fur-IT");
    expect(seen).not.toHaveBeenCalled();
    stop();
  });

  it("keeps working for the session when storage refuses", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(() => setLanguage("fur-IT")).not.toThrow();
    expect(root().lang).toBe("fur-IT");
    // the choice lasts for the session, as the theme's does: a fresh page (`resetLanguage`) has nothing
    expect(getLanguage()).toBe("fur-IT");
    resetLanguage();
    expect(getLanguage()).toBeNull();
  });
});

describe("applyLanguage", () => {
  it("gives the page's own tag back for no choice", () => {
    applyLanguage("fr-FR");
    expect(root().lang).toBe("fr-FR");
    applyLanguage(null);
    expect(root().lang).toBe(PAGE_LANGUAGE);
  });
});

/** Runs the head script against a fake document: what it leaves on the root, before any React. */
function runScript(stored: string | null | undefined, storageThrows = false) {
  const page = { lang: PAGE_LANGUAGE };
  new Function("document", "localStorage", LANGUAGE_SCRIPT)(
    { documentElement: page },
    {
      getItem() {
        if (storageThrows) throw new Error("blocked");
        return stored;
      },
    },
  );
  return page.lang;
}

describe("LANGUAGE_SCRIPT (the head script, before the first paint)", () => {
  it("sets the stored tag on the same rule as isInterfaceTag, and leaves the page's own tag otherwise", () => {
    for (const tag of ACCEPTED) expect(runScript(tag), tag).toBe(tag);
    for (const tag of REFUSED) expect(runScript(tag), String(tag)).toBe(PAGE_LANGUAGE);
  });

  it("leaves the page's own tag when storage is refused", () => {
    expect(runScript("fr-FR", true)).toBe(PAGE_LANGUAGE);
  });

  it("is a constant with nothing of the input in it", () => {
    expect(LANGUAGE_SCRIPT).toContain(JSON.stringify(LANGUAGE_KEY));
    expect(LANGUAGE_SCRIPT).not.toMatch(/eval|innerHTML|document\.write|import/);
  });
});

describe("languageName", () => {
  it("names the four offered languages in their own language", () => {
    expect(DEFAULT_LANGUAGES).toEqual(["en-GB", "fr-FR", "gsw-CH", "fur-IT"]);
    expect(DEFAULT_LANGUAGES.map(languageName)).toEqual([
      "English (United Kingdom)",
      "Français (France)",
      "Schweizerdeutsch (Schweiz)",
      "Furlan (Italie)",
    ]);
    expect(Object.keys(LANGUAGE_NAMES)).toEqual([...DEFAULT_LANGUAGES]);
  });

  it("draws a tag it has no name for as the tag itself, so a deployment's own list works", () => {
    expect(languageName("de-DE")).toBe("de-DE");
    expect(languageName("zh-Hant-TW")).toBe("zh-Hant-TW");
    // a tag that is also a member of Object.prototype is still just a tag
    expect(languageName("toString")).toBe("toString");
  });
});
