/*
 * The interface language of the page: the language the person picks, kept on this device (localStorage
 * `chat.lang`; ADR 0085; web/README.md "The interface language"), and put on `<html lang>` before the first
 * paint. It is a display preference of this client and of nothing else: no API field carries it, and another
 * client keeps its own. The send path reads the page's tag as it always did (`features/locale/lib/locale.ts`,
 * `forwardedProps["vymalo.locale"]`), so a choice changes what messages say without touching that code.
 *
 * The interface itself is **not translated**: picking a language says which language the person reads, and
 * what their messages tell the agent; the words of this app stay English. Each language is therefore drawn by
 * its own name (`languageName`), so a person finds theirs whatever the interface says.
 *
 * `LANGUAGE_SCRIPT` sets the attribute in `<head>` before the first paint (the layout allows it by its hash,
 * `lib/csp.ts`); the functions below keep it right afterwards. The script cannot import, so it carries the tag
 * rule as a literal regex; `language.test.ts` pins it and `isInterfaceTag` to the same answers, so the two
 * cannot drift silently.
 */

import { INTERFACE_TAG_SOURCE, isInterfaceTag, MAX_LOCALE_LEN } from "@/features/locale/lib/locale";

/** The localStorage key, in the pattern of the theme's (`chat.theme`). */
export const LANGUAGE_KEY = "chat.lang";

/**
 * The languages a deployment offers when its configuration names none (`ui.languages` absent: an older
 * orchestrator), in the order the picker draws them. Each is a BCP 47 tag `isInterfaceTag` accepts.
 */
export const DEFAULT_LANGUAGES: readonly string[] = ["en-GB", "fr-FR", "gsw-CH", "fur-IT"];

/**
 * The name of each offered language, in that language and in the words of its own region: the interface is
 * not translated, so a name in English would be a language nobody there calls it by. A tag with no entry is
 * drawn as the tag itself (`languageName`), so a deployment's own list still works.
 */
export const LANGUAGE_NAMES: Readonly<Record<string, string>> = {
  "en-GB": "English (United Kingdom)",
  "fr-FR": "Français (France)",
  "gsw-CH": "Schweizerdeutsch (Schweiz)",
  "fur-IT": "Furlan (Italie)",
};

/**
 * The tag the export is drawn in, before anyone chooses (`<html lang>` in `app/layout.tsx`): the page's own
 * language, and not one of the offered ones. A cleared choice goes back to it.
 */
export const PAGE_LANGUAGE = "en";

/** The longest tag the script accepts, in bytes: the send path's own bound, so the two cannot differ. */
const MAX_TAG_LEN = MAX_LOCALE_LEN;

/** The name of `tag` in its own language, or the tag itself when this app has no name for it. */
export function languageName(tag: string): string {
  return Object.hasOwn(LANGUAGE_NAMES, tag) ? (LANGUAGE_NAMES[tag] ?? tag) : tag;
}

/**
 * The stored tag, or nothing for a value this client would not send: the same rule as
 * `isInterfaceTag`, so a page whose storage holds a malformed tag keeps the page's own and sends it.
 */
export function parseLanguage(raw: string | null | undefined): string | null {
  return typeof raw === "string" && isInterfaceTag(raw) ? raw : null;
}

// What this page remembers: storage, read once, and what a choice makes even when storage refuses (it then
// lasts for the session, as the theme's does). `undefined` is "not read yet".
let session: string | null | undefined;

/** The language the person chose on this device, or nothing until they chose. */
export function getLanguage(): string | null {
  if (typeof document === "undefined") return null;
  if (session === undefined) {
    try {
      session = parseLanguage(localStorage.getItem(LANGUAGE_KEY));
    } catch {
      // storage refused: nothing was chosen
      session = null;
    }
  }
  return session;
}

/** Forgets what was read, for tests (a fresh page). */
export function resetLanguage(): void {
  session = undefined;
}

/**
 * Puts the tag on the page (`<html lang>`), or the page's own for no choice. A write of what is already there
 * is skipped: the head script has set it, and a second write is a mutation a watcher would see.
 */
export function applyLanguage(tag: string | null): void {
  if (typeof document === "undefined") return;
  const lang = tag ?? PAGE_LANGUAGE;
  if (document.documentElement.lang !== lang) document.documentElement.lang = lang;
}

/**
 * Runs in `<head>` before the first paint, like `THEME_SCRIPT`: puts the stored tag on `<html>`, so the run a
 * message goes out in reads the right one from the first byte. Storage may be refused, and a tag this client
 * would not send is no choice: the page keeps the export's own tag. The same rule as `parseLanguage`
 * (`language.test.ts` runs both against `isInterfaceTag`).
 */
export const LANGUAGE_SCRIPT = `(function(){var s=null;try{s=localStorage.getItem(${JSON.stringify(LANGUAGE_KEY)})}catch(e){}if(typeof s==="string"&&s.length<=${MAX_TAG_LEN}&&/${INTERFACE_TAG_SOURCE}/.test(s))document.documentElement.lang=s})()`;

const listeners = new Set<() => void>();
let detach: (() => void) | undefined;

const notify = () => {
  for (const fn of [...listeners]) fn();
};

/** Chooses a language: shown at once, kept on this device (when the browser lets it) and told to the subscribers. */
export function setLanguage(tag: string): void {
  const valid = parseLanguage(tag);
  // a tag the send path would refuse is no choice at all: nothing is written, and the page keeps its own
  if (valid === null) return;
  session = valid;
  try {
    localStorage.setItem(LANGUAGE_KEY, valid);
  } catch {
    // storage refused: the choice lasts for this session
  }
  applyLanguage(valid);
  notify();
}

/**
 * Follows a choice made in another tab. Returns the way to stop; the first subscriber starts the listening,
 * the last one ends it. The value is read again from storage (another tab's choice, or a cleared one), so this
 * only has to put it on the page and tell the subscribers.
 */
export function subscribeLanguage(listener: () => void): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    const onStorage = (event: StorageEvent) => {
      // a key of something else is not ours; no key at all is `localStorage.clear()` in another tab
      if (event.key !== null && event.key !== LANGUAGE_KEY) return;
      session = undefined;
      applyLanguage(getLanguage());
      notify();
    };
    window.addEventListener("storage", onStorage);
    detach = () => window.removeEventListener("storage", onStorage);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      detach?.();
      detach = undefined;
    }
  };
}
