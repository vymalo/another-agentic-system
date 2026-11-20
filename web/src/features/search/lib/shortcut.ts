import { useEffect } from "react";

/** Fired on `window` when the search box should take the focus (and, where it is hidden, be shown first). */
export const SEARCH_FOCUS_EVENT = "another-agentic:search-focus";

/** The sidebar is a column from this width up (Tailwind `md`), and a sheet below it. */
const DESKTOP = "(min-width: 768px)";

/** How long a request to focus waits for a box that is not mounted yet (a sheet that is opening). */
const PENDING_MS = 1_000;

let requestedAt = Number.NEGATIVE_INFINITY;
let returnTarget: HTMLElement | null = null;

/** Whether the page lays the threads out as a sidebar (a desktop) rather than a sheet (a phone). */
export function sidebarIsColumn(): boolean {
  return typeof window !== "undefined" && window.matchMedia(DESKTOP).matches;
}

/** Apple platforms use ⌘; elsewhere Ctrl. `navigator.platform` is deprecated but the one signal that is everywhere. */
export function isApple(): boolean {
  if (typeof navigator === "undefined") return false;
  return /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent);
}

/**
 * Ctrl or ⌘ with K, nothing else held. On an Apple platform Ctrl+K is the text field's own "delete to the end of the line",
 * so only ⌘ counts there; elsewhere Ctrl does, and so does a Meta key.
 */
export function isSearchShortcut(e: KeyboardEvent): boolean {
  if (e.isComposing || e.shiftKey || e.altKey) return false;
  if (e.key.toLowerCase() !== "k") return false;
  return isApple() ? e.metaKey && !e.ctrlKey : e.ctrlKey || e.metaKey;
}

/**
 * Asks for the search box: remembers where the focus was (Escape on an empty box gives it back) and tells whoever has to show the
 * box first (the collapsed sidebar, the phone's sheet) and the box itself.
 */
export function requestSearchFocus(): void {
  const active = document.activeElement;
  if (active instanceof HTMLElement && !active.closest('search, [role="search"]'))
    returnTarget = active;
  requestedAt = Date.now();
  window.dispatchEvent(new Event(SEARCH_FOCUS_EVENT));
}

/** A request was made a moment ago and no box has answered it yet (a box that mounts now takes it). */
export function focusPending(): boolean {
  return Date.now() - requestedAt < PENDING_MS;
}

/** The request was answered. */
export function focusDone(): void {
  requestedAt = Number.NEGATIVE_INFINITY;
}

/** Where the focus was when the shortcut was pressed, if it is still on the page; forgotten once taken. */
export function takeReturnTarget(): HTMLElement | null {
  const target = returnTarget?.isConnected ? returnTarget : null;
  returnTarget = null;
  return target;
}

/** The one listener for the shortcut, in the shell. It is not taken from a dialog (the dialog has the focus on purpose). */
export function useSearchShortcut(enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || !isSearchShortcut(e)) return;
      // a dialog has the focus on purpose, except the thread list's own sheet, whose box the shortcut means
      const dialog =
        e.target instanceof Element
          ? e.target.closest('[role="dialog"], [role="alertdialog"]')
          : null;
      if (dialog && !dialog.hasAttribute("data-threads-sheet")) return;
      e.preventDefault();
      requestSearchFocus();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [enabled]);
}

/** Runs `handler` for every request for the search box. */
export function useSearchFocusRequests(handler: () => void): void {
  useEffect(() => {
    window.addEventListener(SEARCH_FOCUS_EVENT, handler);
    return () => window.removeEventListener(SEARCH_FOCUS_EVENT, handler);
  }, [handler]);
}
