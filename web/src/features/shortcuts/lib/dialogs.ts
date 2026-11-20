/**
 * A dialog on the page — a question, a share, the shortcuts sheet, and every popover too, since Radix gives one
 * `role="dialog"` as well: Esc belongs to it first, and it closes.
 *
 * One constant for the two places that read it: the speech store (Esc stops a read only when nothing else claims it) and
 * the shortcuts sheet (`?` does not stack a second dialog over an open one).
 */
export const OPEN_DIALOG =
  '[role="dialog"][data-state="open"], [role="alertdialog"][data-state="open"]';
