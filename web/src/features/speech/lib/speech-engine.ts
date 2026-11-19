import { chunkSpeech } from "./speech-text";

/*
 * The device's own voice (ADR 0067 S1): `window.speechSynthesis` and `SpeechSynthesisUtterance`, the voice the browser
 * and the operating system already have. No network, no server, no key and no setting — the deployment's voice is S2/S3.
 *
 * An answer is said as several utterances, one per chunk (`chunkSpeech`), because a browser says an utterance and stops
 * at its end: a 20,000-character answer spoken as one would be cut off or refused. The queue is the browser's own; Stop
 * is `cancel()`, and what the browser says of an utterance that was cancelled is ignored.
 *
 * **The queue is filled one task later.** Chromium can drop an utterance queued in the same task as `cancel()`, and the
 * store stops the voice before every answer (it cannot know whether the device still holds a queue). Filling it from a
 * `setTimeout(…, 0)` puts the two in different tasks, and the run check inside it makes a `stop()` in between inert.
 * *Unverified* on a real device: no browser with a speech engine was heard here.
 *
 * This module is the only place that touches the browser's speech API, so a test can stand in for the voice
 * (`SpeechEngine`) and the store (`hooks/use-speech.ts`) holds no browser type.
 */

/** How a voice says an answer ended, or that it would not say it. */
export type SpeechHandlers = {
  /** The last chunk was said. */
  end: () => void;
  /** The browser refused or failed: `reason` is its own word for why (`not-allowed`, `synthesis-failed`, …). */
  error: (reason: string) => void;
};

/** What the page needs of a voice: say these words, and stop. */
export type SpeechEngine = {
  /** Says `words`, in order, and reports through `handlers` when it ends or refuses. */
  speak: (words: string, handlers: SpeechHandlers) => void;
  /** Cuts what is being said and drops what is queued; nothing is reported afterwards. */
  stop: () => void;
};

/**
 * The device's voice, or null where the browser has none (a page rendered on the server, a static export read by a
 * browser without speech, a webview that removed it): with null, no "Read aloud" is drawn at all.
 */
export function deviceSpeechEngine(): SpeechEngine | null {
  if (typeof window === "undefined") return null;
  const synth = window.speechSynthesis;
  const Utterance = window.SpeechSynthesisUtterance;
  if (!synth || typeof Utterance !== "function") return null;
  // one run at a time: a stopped run's utterances report nothing, whatever the browser says of them afterwards
  let run = 0;
  return {
    speak(words, handlers) {
      const mine = ++run;
      const chunks = chunkSpeech(words);
      if (chunks.length === 0) {
        handlers.end();
        return;
      }
      // a task of its own, so a `cancel()` in this one cannot swallow the queue (see the module's comment); the check
      // inside makes a `stop()` in between inert
      setTimeout(() => {
        if (mine !== run) return;
        chunks.forEach((chunk, index) => {
          const utterance = new Utterance(chunk);
          // the page's own language, so the device picks a voice that reads it
          if (typeof document !== "undefined" && document.documentElement.lang) {
            utterance.lang = document.documentElement.lang;
          }
          utterance.onend = () => {
            if (mine === run && index === chunks.length - 1) handlers.end();
          };
          utterance.onerror = (event) => {
            if (mine === run) handlers.error(event.error);
          };
          synth.speak(utterance);
        });
      }, 0);
    },
    stop() {
      run += 1;
      synth.cancel();
    },
  };
}
