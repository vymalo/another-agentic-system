import { isApple } from "@/features/search/lib/shortcut";

/*
 * The keys this page listens to, in one place: the help sheet draws them (`components/shortcuts-sheet.tsx`) and
 * `shortcuts.test.ts` holds each chord against the handler that reads it, so a binding the page does not have cannot be
 * listed here and a chord the page has cannot be listed wrong. web/README.md quotes the same list (a Markdown page
 * cannot import it) and says so. Only bindings that exist are listed — a key that does nothing is worse than no help.
 *
 * Where each one lives:
 * - Search threads: `features/search/lib/shortcut.ts` (`useSearchShortcut`)
 * - Thread details (the panel): `features/panel/lib/shortcut.ts` (`isPanelShortcut`)
 * - Send, New line and Stop and send: the composer, through `features/chat/lib/send.ts` (`modeOfKey`)
 * - Close a dialog, or stop reading aloud: Radix's dialogs, and `features/speech/hooks/use-speech.ts` (`isEscapeStop`)
 * - Show these keys: the sheet's own listener (`isHelpKey` below)
 */

/** One key of a chord, as the page reads it: a modifier and a character, never a physical code (the layout decides it). */
export type ShortcutKey = {
  /** The character, as `KeyboardEvent.key` says it ("K", ".", "Enter", "Escape"). */
  key: string;
  /** Ctrl on anything but an Apple platform, ⌘ on one. */
  mod?: boolean;
  shift?: boolean;
  alt?: boolean;
};

/** A shortcut the page listens to, as the sheet lists it: one chord, or the several it can be pressed with. */
export type Shortcut = {
  id: string;
  /** What it does, in the page's own words. */
  what: string;
  keys: readonly ShortcutKey[];
};

export const SHORTCUTS: readonly Shortcut[] = [
  { id: "search", what: "Search threads", keys: [{ key: "K", mod: true }] },
  {
    id: "panel",
    what: "Thread details (the panel)",
    keys: [{ key: ".", mod: true, shift: true }],
  },
  { id: "send", what: "Send", keys: [{ key: "Enter" }] },
  { id: "new-line", what: "New line", keys: [{ key: "Enter", shift: true }] },
  { id: "stop-and-send", what: "Stop and send", keys: [{ key: "Enter", mod: true, shift: true }] },
  { id: "escape", what: "Close a dialog, or stop reading aloud", keys: [{ key: "Escape" }] },
  { id: "help", what: "Show these keys", keys: [{ key: "?" }, { key: "/", mod: true }] },
];

/** How a key is drawn: `KeyboardEvent.key` is what the page reads, not always what a keyboard shows. */
const SHOWN: Record<string, string> = { Escape: "Esc" };

/** The keys of a chord, in the order a person presses them, with the platform's own modifier (`isApple`). */
export function keyParts(key: ShortcutKey, apple = isApple()): string[] {
  const parts: string[] = [];
  if (key.mod) parts.push(apple ? "⌘" : "Ctrl");
  if (key.shift) parts.push(apple ? "⇧" : "Shift");
  if (key.alt) parts.push(apple ? "⌥" : "Alt");
  parts.push(SHOWN[key.key] ?? key.key);
  return parts;
}

/** Whether a person is writing somewhere: `?` is a character there, not a request for help. */
export function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  return target.isContentEditable || target.closest("[contenteditable]") !== null;
}

/**
 * The help sheet's own keys: **Ctrl/⌘+`/`** (in a box, but not over a dialog that is already open — the browser binds
 * neither on any platform we know) and **`?`** (the character Shift+/ on most layouts, in a box it is just a character).
 * The Shift is not part of the `/` chord: on AZERTY `/` is the shifted character of its key, so Ctrl+Shift+`/` arrives
 * with `shiftKey` held and must open the sheet too. Neither is taken with a modifier that makes it something else (⌥
 * produces another character on many layouts). Nothing else opens it: no key of the page changes because the sheet
 * exists.
 */
export function isHelpKey(event: {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  isComposing: boolean;
}): boolean {
  if (event.isComposing) return false;
  if (event.key === "/") {
    return (event.ctrlKey || event.metaKey) && !event.altKey;
  }
  if (event.key === "?") return !event.ctrlKey && !event.metaKey && !event.altKey;
  return false;
}
