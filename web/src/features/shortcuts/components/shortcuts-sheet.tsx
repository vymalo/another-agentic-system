"use client";

import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { isApple } from "@/features/search/lib/shortcut";
import { OPEN_DIALOG } from "../lib/dialogs";
import { isHelpKey, isTyping, keyParts, SHORTCUTS, type ShortcutKey } from "../lib/shortcuts";

/** One chord: each key in its own `<kbd>`, joined by `+` where the platform's modifiers are words ("Ctrl + Shift + ."). */
function Keys({ chord }: { chord: ShortcutKey }) {
  const apple = isApple();
  return (
    <span className="flex flex-wrap items-center gap-1">
      {keyParts(chord, apple).map((part, index) => (
        <Fragment key={part}>
          {index > 0 ? (
            <span aria-hidden="true" className="text-muted-foreground">
              {apple ? "" : "+"}
            </span>
          ) : null}
          <kbd className="rounded border bg-muted px-1.5 py-0.5 font-mono text-xs text-foreground">
            {part}
          </kbd>
        </Fragment>
      ))}
    </span>
  );
}

/** Every chord of a shortcut, one after another, joined by "or" where the page takes more than one. */
function Chords({ chords }: { chords: readonly ShortcutKey[] }) {
  return (
    <span className="flex flex-wrap items-center gap-1.5">
      {chords.map((chord, index) => (
        <Fragment key={`${chord.key}:${chord.mod ?? false}:${chord.shift ?? false}`}>
          {index > 0 ? <span className="text-xs text-muted-foreground">or</span> : null}
          <Keys chord={chord} />
        </Fragment>
      ))}
    </span>
  );
}

/**
 * The help sheet (ADR 0067 S1): the keys this page listens to, opened by `?` and by Ctrl/⌘+`/` — and by nothing else,
 * because the owner asked for no new binding beyond the sheet itself. `?` is not read while a person writes in a box (it
 * is a character there) nor while a dialog is open (Esc closes that one first); Ctrl/⌘+`/` is read in a box, but not
 * over a dialog that is already open, since no browser binds it.
 *
 * It is a real modal dialog: Radix moves the focus into it, gives it back where it was when it closes, and closes it on
 * Esc. The list is `SHORTCUTS` (`../lib/shortcuts.ts`), the same array web/README.md quotes, so the sheet and the
 * documentation cannot say different things — and it lists its own two openers as a row, so the description promises
 * nothing the page does not do.
 */
export function ShortcutsSheet() {
  const [open, setOpen] = useState(false);
  // where the focus was when the sheet opened: Radix has no trigger to give it back to here
  const returnTo = useRef<HTMLElement | null>(null);
  const openSheet = useCallback(() => {
    returnTo.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setOpen(true);
  }, []);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || !isHelpKey(event)) return;
      // `?` is a character in a box, and neither key opens a second dialog over one that is open
      if (event.key === "?" && isTyping(event.target)) return;
      if (document.querySelector(OPEN_DIALOG)) return;
      event.preventDefault();
      openSheet();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [openSheet]);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent
        data-slot="shortcuts-sheet"
        aria-modal="true"
        className="max-w-md"
        onCloseAutoFocus={(event) => {
          // the focus goes back where the person was, as every dialog of the page does
          event.preventDefault();
          returnTo.current?.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle>Keyboard shortcuts</DialogTitle>
          <DialogDescription>The keys this page listens to, and no others.</DialogDescription>
        </DialogHeader>
        <dl className="grid grid-cols-[1fr_auto] items-center gap-x-6 gap-y-2.5">
          {SHORTCUTS.map((shortcut) => (
            <Fragment key={shortcut.id}>
              <dt className="text-sm text-foreground">{shortcut.what}</dt>
              <dd className="justify-self-end">
                <Chords chords={shortcut.keys} />
              </dd>
            </Fragment>
          ))}
        </dl>
      </DialogContent>
    </Dialog>
  );
}
