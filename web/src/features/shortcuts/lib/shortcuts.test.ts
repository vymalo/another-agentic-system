// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { modeOfKey } from "@/features/chat/lib/send";
import { isPanelShortcut } from "@/features/panel/lib/shortcut";
import { isSearchShortcut } from "@/features/search/lib/shortcut";
import { isEscapeStop } from "@/features/speech/hooks/use-speech";
import { isHelpKey, isTyping, keyParts, SHORTCUTS, type ShortcutKey } from "./shortcuts";

/*
 * The help sheet lists the keys this page really listens to, so the list is held against the handlers themselves: a chord
 * in the list is one the handler accepts, and the platform's modifier is the one the handlers read (⌘ on Apple, Ctrl
 * elsewhere). The sheet's own keys are held against `isHelpKey`: `?` is a character in a box, Ctrl/⌘+`/` is not (and it
 * is Shift+/ on a layout like AZERTY, so the Shift is not part of the chord), and nothing is read with a modifier that
 * makes it another character.
 */

/** A keyboard event of the page's shape, with the physical code a layout gives a chord's key. */
const eventOf = (
  key: string,
  {
    mod = false,
    shift = false,
    apple = false,
  }: { mod?: boolean; shift?: boolean; apple?: boolean } = {},
) => {
  const code =
    key === "."
      ? "Period"
      : key === "Enter"
        ? "Enter"
        : key === "Escape"
          ? "Escape"
          : key === "/"
            ? "Slash"
            : `Key${key.toUpperCase()}`;
  return {
    key,
    code,
    shiftKey: shift,
    altKey: false,
    isComposing: false,
    ctrlKey: mod && !apple,
    metaKey: mod && apple,
  };
};

/** jsdom is not an Apple platform; this says so once, for the help row's `Ctrl`. */
const isApplePlatform = () =>
  /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent);

/** Every chord of every row, as the handlers would see it. */
const chords = SHORTCUTS.flatMap((s) => s.keys.map((key) => ({ id: s.id, key })));
const search = SHORTCUTS.find((s) => s.id === "search");
const panel = SHORTCUTS.find((s) => s.id === "panel");
const closeOrStop = SHORTCUTS.find((s) => s.id === "escape");
const help = SHORTCUTS.find((s) => s.id === "help");

describe("the shortcuts the page listens to", () => {
  it("are the seven the page has, each once", () => {
    expect(SHORTCUTS.map((s) => s.what)).toEqual([
      "Search threads",
      "Thread details (the panel)",
      "Send",
      "New line",
      "Stop and send",
      "Close a dialog, or stop reading aloud",
      "Show these keys",
    ]);
    expect(new Set(SHORTCUTS.map((s) => s.id)).size).toBe(SHORTCUTS.length);
  });

  it("Search threads is the chord the search handler reads, on both platforms", () => {
    expect(search?.keys).toEqual([{ key: "K", mod: true }]);
    expect(isSearchShortcut(eventOf("K", { mod: true }) as KeyboardEvent)).toBe(true);
    expect(isSearchShortcut(eventOf("K", { mod: true, apple: true }) as KeyboardEvent)).toBe(true);
    // and no other chord of the list is the search box's
    for (const chord of chords.filter((c) => c.id !== "search")) {
      expect(isSearchShortcut(eventOf(chord.key.key, { ...chord.key }) as KeyboardEvent)).toBe(
        false,
      );
    }
  });

  it("Thread details is the chord the panel handler reads, on both platforms", () => {
    expect(panel?.keys).toEqual([{ key: ".", mod: true, shift: true }]);
    expect(isPanelShortcut(eventOf(".", { mod: true, shift: true }))).toBe(true);
    expect(isPanelShortcut(eventOf(".", { mod: true, shift: true, apple: true }))).toBe(true);
    expect(isPanelShortcut(eventOf(".", { mod: true }))).toBe(false);
    for (const chord of chords.filter((c) => c.id !== "panel")) {
      expect(isPanelShortcut(eventOf(chord.key.key, { ...chord.key }))).toBe(false);
    }
  });

  it("Stop and send is the chord the composer reads while the agent works", () => {
    const stop = SHORTCUTS.find((s) => s.id === "stop-and-send");
    expect(modeOfKey(eventOf(stop?.keys[0]?.key ?? "", { ...stop?.keys[0] }))).toBe("interrupt");
    // Send steers, and Shift+Enter is a new line and not a send
    expect(modeOfKey(eventOf("Enter"))).toBe("steer");
    expect(modeOfKey(eventOf("Enter", { shift: true }))).toBeNull();
  });

  it("Close a dialog, or stop reading aloud is the key the speech store reads", () => {
    // the row stores a `KeyboardEvent.key`, which is "Escape"; the sheet draws "Esc"
    expect(closeOrStop?.keys).toEqual([{ key: "Escape" }]);
    expect(isEscapeStop({ key: "Escape", defaultPrevented: false })).toBe(true);
    // and no other chord of the list stops a read
    for (const chord of chords.filter((c) => c.id !== "escape")) {
      expect(isEscapeStop({ key: chord.key.key, defaultPrevented: false })).toBe(false);
    }
    // Esc does not reach the store when the page has already taken it
    expect(isEscapeStop({ key: "Escape", defaultPrevented: true })).toBe(false);
  });

  it("Show these keys is the two chords the sheet opens on", () => {
    expect(help?.keys).toEqual([{ key: "?" }, { key: "/", mod: true }]);
    const press = (key: ShortcutKey) =>
      isHelpKey({
        key: key.key,
        ctrlKey: key.mod === true && !isApplePlatform(),
        metaKey: false,
        altKey: key.alt === true,
        shiftKey: key.shift === true,
        isComposing: false,
      });
    for (const key of help?.keys ?? []) expect(press(key)).toBe(true);
  });

  it("names the platform's own modifier: ⌘ on Apple, Ctrl elsewhere", () => {
    expect(keyParts({ key: "K", mod: true }, true)).toEqual(["⌘", "K"]);
    expect(keyParts({ key: "K", mod: true }, false)).toEqual(["Ctrl", "K"]);
    expect(keyParts({ key: ".", mod: true, shift: true }, true)).toEqual(["⌘", "⇧", "."]);
    expect(keyParts({ key: ".", mod: true, shift: true }, false)).toEqual(["Ctrl", "Shift", "."]);
    expect(keyParts({ key: "Enter" }, false)).toEqual(["Enter"]);
    expect(keyParts({ key: "Enter", shift: true }, false)).toEqual(["Shift", "Enter"]);
    // the key is a `KeyboardEvent.key`; the sheet draws the name a person reads on their keyboard
    expect(keyParts({ key: "Escape" }, true)).toEqual(["Esc"]);
  });
});

describe("the help sheet's own keys", () => {
  const press = (over: Partial<Parameters<typeof isHelpKey>[0]>) =>
    isHelpKey({
      key: "",
      ctrlKey: false,
      metaKey: false,
      altKey: false,
      shiftKey: false,
      isComposing: false,
      ...over,
    });

  it("opens with ? (Shift and / on most layouts)", () => {
    expect(press({ key: "?", shiftKey: true })).toBe(true);
  });

  it("opens with Ctrl+/ and with ⌘+/", () => {
    expect(press({ key: "/", ctrlKey: true })).toBe(true);
    expect(press({ key: "/", metaKey: true })).toBe(true);
  });

  it("opens with Ctrl+Shift+/ too: on AZERTY / is the shifted character of that key", () => {
    // there the chord arrives as "/" with Shift held, and a browser binds no Ctrl+Shift+/ of its own
    expect(press({ key: "/", ctrlKey: true, shiftKey: true })).toBe(true);
    expect(press({ key: "/", metaKey: true, shiftKey: true })).toBe(true);
  });

  it("is not taken with a modifier that makes it something else, nor while an IME composes", () => {
    expect(press({ key: "?", altKey: true })).toBe(false);
    expect(press({ key: "?", ctrlKey: true })).toBe(false);
    expect(press({ key: "?", metaKey: true })).toBe(false);
    expect(press({ key: "/", ctrlKey: true, altKey: true })).toBe(false);
    expect(press({ key: "/", altKey: true })).toBe(false);
    // a bare ? is the character itself: a layout that has it unshifted opens the sheet too
    expect(press({ key: "?" })).toBe(true);
    expect(press({ key: "/" })).toBe(false);
    expect(press({ key: "?", isComposing: true })).toBe(false);
    expect(press({ key: "/", ctrlKey: true, isComposing: true })).toBe(false);
  });

  it("knows where a person is writing", () => {
    const of = (tag: string, contenteditable = false) => {
      const el = document.createElement(tag);
      if (contenteditable) el.setAttribute("contenteditable", "true");
      return el;
    };
    expect(isTyping(of("textarea"))).toBe(true);
    expect(isTyping(of("input"))).toBe(true);
    expect(isTyping(of("select"))).toBe(true);
    expect(isTyping(of("div", true))).toBe(true);
    const inside = document.createElement("span");
    of("div", true).append(inside);
    expect(isTyping(inside)).toBe(true);
    expect(isTyping(of("div"))).toBe(false);
    expect(isTyping(document.body)).toBe(false);
    expect(isTyping(null)).toBe(false);
  });
});
