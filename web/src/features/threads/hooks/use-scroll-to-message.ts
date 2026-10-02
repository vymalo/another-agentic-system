import { useEffect } from "react";

/** How long the page waits for the message a link names to be drawn before it gives up. */
const GIVE_UP_MS = 10_000;

/**
 * A link to a message of a thread is `/threads/<id>#m-<seq>` (the versions of a message, an edit
 * just sent): the messages are drawn a moment after the page opens, from the replay of the log, so
 * the browser's own jump to the id finds nothing. This waits for `#m-<seq>` (the user bubbles carry
 * that id), scrolls it into the middle of the view, once, and puts the focus on it, so that a
 * screen reader reads where it was taken and Tab goes on from there. A message that never comes
 * (a link to an event that is not a message) is given up after a few seconds, and the page stays
 * as it is. Nothing happens without the hash, and a person who has scrolled away is not pulled back.
 */
export function useScrollToMessage(threadId: string | null, loaded: boolean): void {
  useEffect(() => {
    if (threadId === null || !loaded) return;
    const id = /^#(m-\d+)$/.exec(window.location.hash)?.[1];
    if (!id) return;
    let frame = 0;
    let observer: MutationObserver | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const stop = () => {
      cancelAnimationFrame(frame);
      observer?.disconnect();
      clearTimeout(timer);
      window.removeEventListener("wheel", stop);
      window.removeEventListener("touchmove", stop);
    };
    const look = (): boolean => {
      const el = document.getElementById(id);
      if (!el) return false;
      // a frame later: the transcript has been laid out, and the viewport's own pull to the end has run
      frame = requestAnimationFrame(() => {
        el.scrollIntoView({ block: "center" });
        el.focus({ preventScroll: true });
      });
      return true;
    };
    if (look()) return stop;
    observer = new MutationObserver(() => {
      if (look()) {
        observer?.disconnect();
        clearTimeout(timer);
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
    timer = setTimeout(stop, GIVE_UP_MS);
    window.addEventListener("wheel", stop, { passive: true });
    window.addEventListener("touchmove", stop, { passive: true });
    return stop;
  }, [threadId, loaded]);
}
