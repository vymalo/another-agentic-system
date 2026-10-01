/**
 * Scrolls the chat to an agent turn and puts the focus on its header, so a screen reader and the
 * keyboard continue from there. The turn is the element the transcript marks `data-turn-id`
 * (thread.aui.tsx); a turn that is not on the page (a message that draws nothing) is nothing.
 * Reduced motion is the stylesheet's (`scroll-smooth` is off under it).
 */
export function focusTurn(turnId: string, root: ParentNode = document): boolean {
  const turn = [...root.querySelectorAll<HTMLElement>("[data-turn-id]")].find(
    (el) => el.dataset.turnId === turnId,
  );
  if (!turn) return false;
  turn.scrollIntoView({ block: "start" });
  turn.querySelector<HTMLElement>('[data-slot="turn-header"]')?.focus({ preventScroll: true });
  return true;
}
