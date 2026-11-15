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
 * Every piece drawn starts the wait again.
 */
const SETTLE_MS = 600;

/** The person is typing in a field that is not the search box. */
function typingElsewhere(): boolean {
  const el = document.activeElement;
  if (!(el instanceof HTMLElement)) return false;
  if (el.dataset.slot === "search-input") return false;
  return (
    el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el.isContentEditable
  );
}

/** The last person's message at or before `seq` (their elements carry `data-seq`), if one is drawn. */
function nearest(seq: number): HTMLElement | null {
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
 * screen reader reads where it was taken and Tab goes on from there. A link whose message never comes (and that has no
 * earlier person's message to stand for it) is given up after ten seconds, and the page stays as it is. Nothing happens
 * without the hash, and a person who has scrolled away is not pulled back.
 *
 * Only a person's message has an element with that id. For any other `#m-<seq>` (a search hit in an agent's answer, ADR 0068; a
 * shared link; a link to an edited or deleted message) the page goes, once the transcript has been still for 600 ms (every drawn
 * piece starts the wait again), to the last person's message at or before it: the one that started the turn the event belongs to.
 * It keeps watching until the give-up, and moves to the exact message if it is drawn later. The focus is taken only when the person
 * is not typing in another field (the search box is not "another field": a hit chosen there is meant to take it). The hash changing
 * on the open page (a hit of this thread chosen in the search) goes again.
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
    let landed: HTMLElement | null = null;
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
    const go = (el: HTMLElement) => {
      landed = el;
      cancelAnimationFrame(frame);
      clearTimeout(release);
      // two frames later: the transcript has been laid out, and so has the viewport's own pull to the end, which runs when
      // the size of what it holds changes and would take a scroll made before it back to the end. Instant: the person
      // arrives at the message, they do not watch the page travel to it.
      setPinned(true);
      frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(() => {
          el.scrollIntoView({ block: "center", behavior: "instant" });
          // a person who is typing somewhere else keeps the focus (the search box does not count: it chose this hit)
          if (!typingElsewhere()) el.focus({ preventScroll: true });
          release = setTimeout(() => setPinned(false), PIN_MS);
        });
      });
    };
    /** The exact message: done. Otherwise, once settled, the nearest earlier one, and the watch goes on for the exact one. */
    const look = (settled: boolean): boolean => {
      const exact = document.getElementById(`m-${seq}`);
      if (exact) {
        if (landed !== exact) go(exact);
        return true;
      }
      if (settled && landed === null) {
        const near = nearest(seq);
        if (near) go(near);
      }
      return false;
    };
    const found = () => {
      observer?.disconnect();
      clearTimeout(timer);
      clearTimeout(settle);
    };
    if (look(false)) return stop;
    const wait = () => {
      clearTimeout(settle);
      if (landed !== null) return;
      settle = setTimeout(() => look(true), SETTLE_MS);
    };
    observer = new MutationObserver(() => {
      if (look(false)) found();
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
