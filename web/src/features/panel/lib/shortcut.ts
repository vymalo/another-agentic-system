"use client";

import { useEffect } from "react";

/** Ctrl/⌘+Shift+. toggles the panel: `code`, not `key` (the character depends on the layout), never while an IME composes. */
export function isPanelShortcut(e: {
  code: string;
  shiftKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  isComposing: boolean;
}): boolean {
  if (e.isComposing) return false;
  return e.code === "Period" && e.shiftKey && !e.altKey && (e.ctrlKey || e.metaKey);
}

/** Tells whoever asks (the help sheet's list) that Ctrl/⌘+Shift+. exists, and reads it once for the panel. */
export function usePanelShortcut(toggle: () => void): void {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || !isPanelShortcut(e)) return;
      e.preventDefault();
      toggle();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [toggle]);
}
