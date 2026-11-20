import type { Page } from "@playwright/test";

/*
 * The device's voice, stood in for (ADR 0067 S1): headless Chromium has `speechSynthesis` but no voice to say anything
 * with, so a spec installs its own before the page loads.
 *
 * It is a **queue**, like the browser's: utterances wait their turn, one is in flight, `cancel()` cuts the queue and
 * reports the utterance it cut as `canceled`, and nothing is said or refused until the spec says so. Two quirks of the
 * real thing are modelled because the page has to survive them:
 *
 * - **Chromium drops an utterance queued in the same task as `cancel()`.** The stub drops one too, and says so in its
 *   log, so the page's own mitigation (it fills its queue a task later) is held by a test rather than by a comment.
 *   *Unverified* against Chromium itself: it is the documented shape of the bug, not a measurement.
 * - **A failed chunk does not drop the chunks queued after it.** Some browsers do, some do not; the page stops the queue
 *   itself on a failure, so the stub leaves them for the page to cut.
 *
 * `__failAt(n)` makes the nth chunk fail when it starts, which is how a spec reaches a failure half way through an
 * answer.
 */

/** The stub's own fields on the page's window. */
export type VoiceWindow = Window & {
  /** Every utterance ever queued, in the order they were queued (dropped ones included). */
  __spoken: string[];
  /** How many times the queue was cancelled. */
  __cancels: number;
  /** What the stub was told, in order: a chunk queued, the queue cancelled, a chunk Chromium would have dropped. */
  __log: Array<{ op: "speak" | "cancel" | "dropped"; text?: string }>;
  /** Ends the chunk being said, as the browser does; the next one starts. */
  __end: () => void;
  /** Ends the whole answer, chunk by chunk, as the browser does at the end of the last one. */
  __finish: () => void;
  /** Refuses the chunk being said, as the browser does (`not-allowed`, `synthesis-failed`, …). */
  __fail: (reason: string) => void;
  /** Makes the nth chunk (0-based, as queued) fail when it starts. */
  __failAt: (index: number) => void;
  /** The chunk being said, or null. */
  __speaking: () => string | null;
  /** How many chunks are queued behind the one being said. */
  __pending: () => number;
};

/** Installs a voice that speaks nothing, keeps the queue and waits to be told what happens. */
export async function stubVoice(page: Page): Promise<void> {
  await page.addInitScript(() => {
    type Utterance = {
      text: string;
      lang: string;
      onend: (() => void) | null;
      onerror: ((event: { error: string }) => void) | null;
    };
    const state = window as unknown as VoiceWindow;
    state.__spoken = [];
    state.__cancels = 0;
    state.__log = [];
    let queue: Utterance[] = [];
    let current: Utterance | null = null;
    /** The index each utterance is queued under, so `__failAt` can name one. */
    let queued = 0;
    let failAt: number | null = null;
    /** Set while this task is the one a `cancel()` happened in: Chromium drops what is queued in it. */
    let dropThisTask = false;
    const indices = new WeakMap<Utterance, number>();

    class FakeUtterance implements Utterance {
      text: string;
      lang = "";
      onend: (() => void) | null = null;
      onerror: ((event: { error: string }) => void) | null = null;
      constructor(text: string) {
        this.text = text;
      }
    }

    const start = () => {
      if (current !== null) return;
      const next = queue.shift();
      if (next === undefined) return;
      current = next;
      if (failAt !== null && indices.get(next) === failAt) {
        failAt = null;
        // a browser reports a failure a moment later, never inside `speak`
        const failing = next;
        setTimeout(() => {
          if (current !== failing) return;
          current = null;
          failing.onerror?.({ error: "synthesis-failed" });
        }, 0);
      }
    };

    Object.defineProperty(window, "SpeechSynthesisUtterance", {
      value: FakeUtterance,
      configurable: true,
    });
    Object.defineProperty(window, "speechSynthesis", {
      value: {
        speak: (utterance: Utterance) => {
          state.__spoken.push(utterance.text);
          indices.set(utterance, queued++);
          if (dropThisTask) {
            state.__log.push({ op: "dropped", text: utterance.text });
            return;
          }
          state.__log.push({ op: "speak", text: utterance.text });
          queue.push(utterance);
          start();
        },
        cancel: () => {
          state.__cancels += 1;
          state.__log.push({ op: "cancel" });
          const cut = current;
          current = null;
          queue = [];
          dropThisTask = true;
          setTimeout(() => {
            dropThisTask = false;
          }, 0);
          cut?.onerror?.({ error: "canceled" });
        },
      },
      configurable: true,
    });
    state.__end = () => {
      const said = current;
      current = null;
      said?.onend?.();
      start();
    };
    state.__finish = () => {
      while (current !== null) state.__end();
    };
    state.__fail = (reason: string) => {
      const refused = current;
      current = null;
      refused?.onerror?.({ error: reason });
    };
    state.__failAt = (index: number) => {
      failAt = index;
    };
    state.__speaking = () => current?.text ?? null;
    state.__pending = () => queue.length;
  });
}

/** Takes the voice away, as a browser without one has it. */
export async function removeVoice(page: Page): Promise<void> {
  await page.addInitScript(() => {
    Object.defineProperty(window, "speechSynthesis", { value: undefined, configurable: true });
  });
}

/** What has been said so far, in the order the utterances were queued. */
export async function spoken(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as VoiceWindow).__spoken);
}

/** What the stub was told, in order: `speak`, `cancel`, `dropped`. */
export async function voiceLog(
  page: Page,
): Promise<Array<{ op: "speak" | "cancel" | "dropped"; text?: string }>> {
  return page.evaluate(() => (window as unknown as VoiceWindow).__log);
}

/** How many times the queue was cancelled. */
export async function cancels(page: Page): Promise<number> {
  return page.evaluate(() => (window as unknown as VoiceWindow).__cancels);
}

/** The chunk being said, or null when the queue is empty. */
export async function speaking(page: Page): Promise<string | null> {
  return page.evaluate(() => (window as unknown as VoiceWindow).__speaking());
}

/** How many chunks are queued behind the one being said. */
export async function pending(page: Page): Promise<number> {
  return page.evaluate(() => (window as unknown as VoiceWindow).__pending());
}

/** Ends the chunk that is being read, as the browser does at the end of one; the next one starts. */
export async function endChunk(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as VoiceWindow).__end());
}

/** Ends the answer that is being read, chunk by chunk, as the browser does at its last one. */
export async function endVoice(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as VoiceWindow).__finish());
}

/** Refuses the chunk that is being read, as the browser does. */
export async function failVoice(page: Page, reason = "not-allowed"): Promise<void> {
  await page.evaluate((why) => (window as unknown as VoiceWindow).__fail(why), reason);
}

/** Makes the nth chunk (0-based, as queued) fail when it starts: a failure half way through an answer. */
export async function failChunk(page: Page, index: number): Promise<void> {
  await page.evaluate((at) => (window as unknown as VoiceWindow).__failAt(at), index);
}
