// @vitest-environment jsdom
import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { isSearchShortcut, SEARCH_FOCUS_EVENT, useSearchShortcut } from "./shortcut";

/* Ctrl or ⌘ with K (ADR 0068): when it is taken, and that the default is prevented exactly then. */

const platform = (name: string) =>
  Object.defineProperty(navigator, "platform", { value: name, configurable: true });

/** Dispatches the key; true when a listener prevented its default. */
const press = (init: KeyboardEventInit, target: EventTarget = document.body) =>
  !target.dispatchEvent(
    new KeyboardEvent("keydown", { key: "k", bubbles: true, cancelable: true, ...init }),
  );

afterEach(() => {
  cleanup();
  platform("Linux x86_64");
  document.body.replaceChildren();
});

describe("isSearchShortcut", () => {
  it("takes Ctrl or Meta with K elsewhere, and only Meta on an Apple platform", () => {
    platform("Linux x86_64");
    expect(isSearchShortcut(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }))).toBe(true);
    expect(isSearchShortcut(new KeyboardEvent("keydown", { key: "K", metaKey: true }))).toBe(true);
    platform("MacIntel");
    expect(isSearchShortcut(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }))).toBe(false);
    expect(isSearchShortcut(new KeyboardEvent("keydown", { key: "k", metaKey: true }))).toBe(true);
  });

  it("refuses extra modifiers, other keys and a composition", () => {
    const k = (init: KeyboardEventInit) => isSearchShortcut(new KeyboardEvent("keydown", init));
    expect(k({ key: "k", ctrlKey: true, shiftKey: true })).toBe(false);
    expect(k({ key: "k", ctrlKey: true, altKey: true })).toBe(false);
    expect(k({ key: "j", ctrlKey: true })).toBe(false);
    expect(k({ key: "k", ctrlKey: true, isComposing: true })).toBe(false);
  });
});

describe("useSearchShortcut", () => {
  it("prevents the default and asks for the box exactly when it acts", () => {
    const asked = vi.fn();
    window.addEventListener(SEARCH_FOCUS_EVENT, asked);
    renderHook(() => useSearchShortcut(true));
    expect(press({ ctrlKey: true })).toBe(true);
    expect(asked).toHaveBeenCalledTimes(1);
    expect(press({ key: "j", ctrlKey: true })).toBe(false);
    expect(press({ ctrlKey: true, isComposing: true })).toBe(false);
    expect(asked).toHaveBeenCalledTimes(1);
    window.removeEventListener(SEARCH_FOCUS_EVENT, asked);
  });

  it("is not active when the configuration has no search", () => {
    const asked = vi.fn();
    window.addEventListener(SEARCH_FOCUS_EVENT, asked);
    renderHook(() => useSearchShortcut(false));
    expect(press({ ctrlKey: true })).toBe(false);
    expect(asked).not.toHaveBeenCalled();
    window.removeEventListener(SEARCH_FOCUS_EVENT, asked);
  });

  it("is ignored inside another dialog and taken inside the thread list's own sheet", () => {
    const asked = vi.fn();
    window.addEventListener(SEARCH_FOCUS_EVENT, asked);
    renderHook(() => useSearchShortcut(true));
    document.body.innerHTML =
      '<div role="dialog"><input id="a"></div><div role="dialog" data-threads-sheet=""><input id="b"></div>';
    expect(press({ ctrlKey: true }, document.getElementById("a") as HTMLElement)).toBe(false);
    expect(asked).not.toHaveBeenCalled();
    expect(press({ ctrlKey: true }, document.getElementById("b") as HTMLElement)).toBe(true);
    expect(asked).toHaveBeenCalledTimes(1);
    window.removeEventListener(SEARCH_FOCUS_EVENT, asked);
  });
});
