import { useEffect, useState } from "react";
import { linkedMessage } from "../lib/linked-message";

/** How long the page waits for the message a link names to be drawn before it gives up. */
const GIVE_UP_MS = 10_000;

/**
 * How long the viewport is kept from following the end of the transcript after the message is in view: it pulls a page back to
 * the end whenever what it holds changes size, and a transcript that has just been drawn changes size for a moment.
 */
const PIN_MS = 2_000;

/**
 * A link to a message of a thread is `/threads/<id>#m-<seq>` (the versions of a message, an edit
 * just sent): the messages are drawn a moment after the page opens, from the replay of the log, so
 * the browser's own jump to the id finds nothing. This waits for `#m-<seq>` (the user bubbles carry
 * that id), scrolls it into the middle of the view, once, and puts the focus on it, so that a
 * screen reader reads where it was taken and Tab goes on from there. A message that never comes
 * (a link to an event that is not a message) is given up after a few seconds, and the page stays
 * as it is. Nothing happens without the hash, and a person who has scrolled away is not pulled back.
 *
 * Returns whether the viewport is being kept where the message put it (`Thread`'s `pinned`).
 */
export function useScrollToMessage(threadId: string | null, loaded: boolean): boolean {
  const [pinned, setPinned] = useState(false);
  useEffect(() => {
    if (threadId === null || !loaded) return;
    const seq = linkedMessage();
    if (seq === undefined) return;
    const id = `m-${seq}`;
    let frame = 0;
    let observer: MutationObserver | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let release: ReturnType<typeof setTimeout> | undefined;
    const stop = () => {
      cancelAnimationFrame(frame);
      observer?.disconnect();
      clearTimeout(timer);
      clearTimeout(release);
      setPinned(false);
      window.removeEventListener("wheel", stop);
      window.removeEventListener("touchmove", stop);
    };
    const look = (): boolean => {
      const el = document.getElementById(id);
      if (!el) return false;
      // two frames later: the transcript has been laid out, and so has the viewport's own pull to the end, which runs when
      // the size of what it holds changes and would take a scroll made before it back to the end. Instant: the person
      // arrives at the message, they do not watch the page travel to it.
      setPinned(true);
      frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(() => {
          el.scrollIntoView({ block: "center", behavior: "instant" });
          el.focus({ preventScroll: true });
          release = setTimeout(() => setPinned(false), PIN_MS);
        });
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
  return pinned;
}
