"use client";

import { useSyncExternalStore } from "react";
import { OPEN_DIALOG } from "@/features/shortcuts/lib/dialogs";
import { deviceSpeechEngine, type SpeechEngine } from "../lib/speech-engine";

/*
 * Read aloud is **one answer at a time, app-wide** (ADR 0067 S1): the state lives in a module-level singleton, not in a
 * provider, so every turn of every thread on the page reads the same one, and starting a second answer cuts the first.
 * Nothing is stored and nothing plays by itself: a press starts it, a press (or the end of the words) stops it.
 *
 * The snapshot carries `supported` so the server's render and the page's first render agree: the server has no
 * `speechSynthesis`, so it says false, and React asks again once the page runs and draws the button where there is one.
 * A browser that refuses (for example `not-allowed`) is said in words, and never as an unhandled rejection: the browser
 * reports through an event, or throws, and the store turns either into state.
 *
 * The store is not the device: the device can hold a queue the store no longer knows about — a chunk that failed half
 * way leaves one — so the queue is cut before every answer and on every failure, never only when the store thinks
 * something is read. `stopSpeechIf` is what a turn calls as it leaves the page.
 */

/** What a person is told when the browser would not read an answer aloud. */
export const SPEECH_FAILED = "Could not read aloud: the browser did not allow it.";

/** A refusal, with the answer it is about: it is said under that answer, never on another one. */
export type SpeechFailure = { id: string; message: string };

export type SpeechState = {
  /** Whether this browser has a voice at all. False on the server, so no button is drawn before the page runs. */
  supported: boolean;
  /** The answer that is being read, by the id the page gave it; null when nothing is read. */
  speaking: string | null;
  /** The last refusal and the answer it belongs to; null while nothing was refused since the last attempt. */
  failure: SpeechFailure | null;
};

/** What a turn needs: the state above, and the two things its button does. */
export type Speech = SpeechState & {
  /** Reads this answer, or stops it when it is the one being read. */
  toggle: (id: string, words: string) => void;
  /** Stops whatever is being read. */
  stop: () => void;
};

/** The server's answer, and the first render's: no voice is known yet. */
const SERVER: SpeechState = { supported: false, speaking: null, failure: null };

/** `undefined` until asked, then the device's voice or null: the module is loaded on the server too, and must not touch
 * `window` there. */
let engine: SpeechEngine | null | undefined;
let state: SpeechState | null = null;
/** Bumped by every start and every stop: what an older run reports is not about what is being read now. */
let generation = 0;
const listeners = new Set<() => void>();

/**
 * Whether Esc means "stop reading" here (ADR 0067 S1): the key, not already taken by something else on the page, and no
 * dialog open — a dialog that is open wins, as it always did, and the answer goes on reading. Exported so the help
 * sheet's Esc row is held against the handler that reads it (`features/shortcuts/lib/shortcuts.test.ts`).
 */
export function isEscapeStop(event: { key: string; defaultPrevented: boolean }): boolean {
  if (event.key !== "Escape" || event.defaultPrevented) return false;
  if (typeof document === "undefined" || document.querySelector(OPEN_DIALOG)) return false;
  return true;
}

/**
 * Esc stops what is being read: the way out of a voice without hunting for the button, the way Esc closes what is open
 * everywhere else. Nothing else is bound: no key starts a read, and no key of the page changes.
 */
function onEscape(event: KeyboardEvent): void {
  if (isEscapeStop(event)) stopSpeech();
}

function voice(): SpeechEngine | null {
  if (engine === undefined) engine = deviceSpeechEngine();
  return engine;
}

/** The page's state, read once on the client (the voice is looked for here, never on the server). */
function snapshot(): SpeechState {
  if (state === null) state = { ...SERVER, supported: voice() !== null };
  return state;
}

function emit(next: SpeechState): void {
  const was = state?.speaking ?? null;
  state = next;
  if (typeof document !== "undefined") {
    // Esc is bound only while something is read: nothing of the page's own keys changes
    if (was === null && next.speaking !== null) document.addEventListener("keydown", onEscape);
    else if (was !== null && next.speaking === null)
      document.removeEventListener("keydown", onEscape);
  }
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Stops what is being read; nothing happens when nothing is. */
export function stopSpeech(): void {
  if (snapshot().speaking === null) return;
  generation += 1;
  voice()?.stop();
  emit({ ...snapshot(), speaking: null });
}

/**
 * Stops the answer `id` and only it: what a turn calls when it leaves the page (ADR 0078, "it stops when the answer
 * leaves the page"), so a turn that is scrolled away or replaced does not go on speaking, and a read of another answer
 * is left alone.
 */
export function stopSpeechIf(id: string): void {
  if (snapshot().speaking === id) stopSpeech();
}

/** Reads `words` as the answer `id`, or stops it when it is already being read. */
export function toggleSpeech(id: string, words: string): void {
  const speaking = snapshot().speaking;
  if (speaking === id) {
    stopSpeech();
    return;
  }
  const device = voice();
  // a browser without a voice has no button; an answer that says nothing has nothing to read
  if (device === null || words.trim() === "") return;
  const mine = ++generation;
  // the device is not this store: it can hold a queue the store no longer knows about (a chunk that failed half way
  // leaves one), so it is stopped before every answer and not only when another one is being read
  device.stop();
  emit({ ...snapshot(), speaking: id, failure: null });
  try {
    device.speak(words, {
      end: () => {
        if (mine === generation) emit({ ...snapshot(), speaking: null });
      },
      error: () => {
        if (mine !== generation) return;
        // a browser reports a failure at one chunk and does not always drop the chunks queued after it: the rest of the
        // answer is cut here, so nothing is said after the refusal and nothing is left for Esc to stop
        device.stop();
        emit({ ...snapshot(), speaking: null, failure: { id, message: SPEECH_FAILED } });
      },
    });
  } catch {
    // a voice that throws instead of reporting it: said in words, as the browser's own refusal is
    emit({ ...snapshot(), speaking: null, failure: { id, message: SPEECH_FAILED } });
  }
}

/** Read aloud's state, and what a button does with it. */
export function useSpeech(): Speech {
  const current = useSyncExternalStore(subscribe, snapshot, () => SERVER);
  return { ...current, toggle: toggleSpeech, stop: stopSpeech };
}

/** Stands in for the browser's voice, for tests (`null` is a browser that has none). */
export function setSpeechEngine(next: SpeechEngine | null): void {
  generation += 1;
  engine = next;
  state = null;
}

/** Forgets the voice and what was read, for tests. */
export function resetSpeech(): void {
  generation += 1;
  engine = undefined;
  state = null;
  if (typeof document !== "undefined") document.removeEventListener("keydown", onEscape);
}
