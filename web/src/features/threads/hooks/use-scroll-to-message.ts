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
 * How long the transcript has to be still before the page settles for the nearest earlier person's message: an agent's answer
 * (a search hit, ADR 0068) has no element of its own, and the messages are drawn one after another, so the exact one is waited for.
 */
const SETTLE_MS = 600;

/** The element a link to `seq` goes to: that message's own, or, once `settled`, the last person's message at or before it. */
function target(seq: number, settled: boolean): HTMLElement | null {
  const exact = document.getElementById(`m-${seq}`);
  if (exact || !settled) return exact;
  let best: HTMLElement | null = null;
  let bestSeq = 0;
  for (const el of document.querySelectorAll<HTMLElement>("[data-seq]")) {
    const at = Number(el.dataset.seq);
    if (Number.isSafeInteger(at) && at <= seq && at > bestSeq) {
      best = el;
      bestSeq = at;
    }
  }
  return best;
}

/**
 * A link to a message of a thread is `/threads/<id>#m-<seq>` (the versions of a message, an edit
 * just sent): the messages are drawn a moment after the page opens, from the replay of the log, so
 * the browser's own jump to the id finds nothing. This waits for `#m-<seq>` (the user bubbles carry
 * that id), scrolls it into the middle of the view, once, and puts the focus on it, so that a
 * screen reader reads where it was taken and Tab goes on from there. A message that never comes
 * (a link to an event that is not a message) is given up after a few seconds, and the page stays
 * as it is. Nothing happens without the hash, and a person who has scrolled away is not pulled back.
 *
 * Only a person's message has an element with that id. A link to another event (a search hit in an agent's answer, ADR 0068,
 * names the answer's `seq`) goes, once the transcript has been still for a moment, to the last person's message at or before
 * it: the one that started the turn the event belongs to. The hash changing on the open page (a hit of this thread chosen
 * in the search) goes again.
 *
 * Returns whether the viewport is being kept where the message put it (`Thread`'s `pinned`).
 */
export function useScrollToMessage(threadId: string | null, loaded: boolean): boolean {
  const [pinned, setPinned] = useState(false);
  const [hash, setHash] = useState(0);
  useEffect(() => {
    const again = () => setHash((n) => n + 1);
    window.addEventListener("hashchange", again);
    return () => window.removeEventListener("hashchange", again);
  }, []);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `hash` is the page's own signal that the link changed
  useEffect(() => {
    if (threadId === null || !loaded) return;
    const seq = linkedMessage();
    if (seq === undefined) return;
    let frame = 0;
    let observer: MutationObserver | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let settle: ReturnType<typeof setTimeout> | undefined;
    let release: ReturnType<typeof setTimeout> | undefined;
    const stop = () => {
      cancelAnimationFrame(frame);
      observer?.disconnect();
      clearTimeout(timer);
      clearTimeout(settle);
      clearTimeout(release);
      setPinned(false);
      window.removeEventListener("wheel", stop);
      window.removeEventListener("touchmove", stop);
    };
    const look = (settled = false): boolean => {
      const el = target(seq, settled);
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
    const found = () => {
      observer?.disconnect();
      clearTimeout(timer);
      clearTimeout(settle);
    };
    const wait = () => {
      clearTimeout(settle);
      settle = setTimeout(() => {
        if (look(true)) found();
      }, SETTLE_MS);
    };
    observer = new MutationObserver(() => {
      if (look()) found();
      else wait();
    });
    observer.observe(document.body, { childList: true, subtree: true });
    wait();
    timer = setTimeout(stop, GIVE_UP_MS);
    window.addEventListener("wheel", stop, { passive: true });
    window.addEventListener("touchmove", stop, { passive: true });
    return stop;
  }, [threadId, loaded, hash]);
  return pinned;
}
