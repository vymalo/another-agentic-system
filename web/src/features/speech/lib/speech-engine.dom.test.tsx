// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { deviceSpeechEngine, type SpeechHandlers } from "./speech-engine";

/*
 * The device's own voice (ADR 0067 S1): `window.speechSynthesis` and `SpeechSynthesisUtterance`, and nothing else — no
 * network, no server, no setting. jsdom has neither, so the browser's voice is stood in for here: what matters is that an
 * answer is said as several utterances in order (a browser stops at the end of one), that the answer is over only when
 * the last one ends, that a refusal is reported in the browser's own word for it, and that Stop cancels the queue and
 * reports nothing afterwards — including a Stop in the same task as the Speak, which Chromium can lose.
 */

type Utterance = {
  text: string;
  lang: string;
  onend: (() => void) | null;
  onerror: ((event: { error: string }) => void) | null;
};

let spoken: Utterance[] = [];
let cancelled = 0;

function installVoice({ utterances = true } = {}) {
  spoken = [];
  cancelled = 0;
  class FakeUtterance implements Utterance {
    text: string;
    lang = "";
    onend: (() => void) | null = null;
    onerror: ((event: { error: string }) => void) | null = null;
    constructor(text: string) {
      this.text = text;
    }
  }
  Object.defineProperty(window, "SpeechSynthesisUtterance", {
    value: utterances ? FakeUtterance : undefined,
    configurable: true,
  });
  Object.defineProperty(window, "speechSynthesis", {
    value: {
      speak: (utterance: Utterance) => {
        spoken.push(utterance);
      },
      cancel: () => {
        cancelled += 1;
      },
    },
    configurable: true,
  });
}

/** What the engine reports, and what it was told. */
function handlers() {
  const seen = { ended: 0, failed: [] as string[] };
  return {
    seen,
    handlers: {
      end: () => {
        seen.ended += 1;
      },
      error: (reason: string) => {
        seen.failed.push(reason);
      },
    } satisfies SpeechHandlers,
  };
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  // @ts-expect-error the browser's voice is installed per test
  delete window.speechSynthesis;
  // @ts-expect-error the browser's voice is installed per test
  delete window.SpeechSynthesisUtterance;
});

describe("the device's voice", () => {
  it("is absent where the browser has no speechSynthesis, and where it has no utterance", () => {
    expect(deviceSpeechEngine()).toBeNull();
    installVoice({ utterances: false });
    expect(deviceSpeechEngine()).toBeNull();
  });

  it("says a long answer as its chunks, in order, each as one utterance", () => {
    vi.useFakeTimers();
    installVoice();
    const engine = deviceSpeechEngine();
    expect(engine).not.toBeNull();
    const words = "A sentence of an answer. ".repeat(20).trim();
    engine?.speak(words, handlers().handlers);
    vi.runAllTimers();
    expect(spoken.length).toBeGreaterThan(1);
    expect(spoken.map((u) => u.text).join(" ")).toBe(words);
  });

  it("queues nothing when it was stopped in the same task as the speak", () => {
    vi.useFakeTimers();
    installVoice();
    const { seen, handlers: said } = handlers();
    const engine = deviceSpeechEngine();
    // Chromium can drop an utterance queued in the same task as `cancel()`: the queue is filled a task later, so a Stop
    // between the two leaves nothing to say and nothing to report
    engine?.speak("One. Two.", said);
    engine?.stop();
    vi.runAllTimers();
    expect(spoken).toEqual([]);
    expect(seen.ended).toBe(0);
    expect(seen.failed).toEqual([]);
  });

  it("says the answer is over only when its last utterance ends", () => {
    vi.useFakeTimers();
    installVoice();
    const { seen, handlers: said } = handlers();
    deviceSpeechEngine()?.speak("One. Two.", said);
    vi.runAllTimers();
    expect(spoken).toHaveLength(1);
    spoken[0]?.onend?.();
    expect(seen.ended).toBe(1);
    expect(seen.failed).toEqual([]);
  });

  it("reports the browser's own word when it refuses", () => {
    vi.useFakeTimers();
    installVoice();
    const { seen, handlers: said } = handlers();
    deviceSpeechEngine()?.speak("Words.", said);
    vi.runAllTimers();
    spoken[0]?.onerror?.({ error: "not-allowed" });
    expect(seen.failed).toEqual(["not-allowed"]);
    expect(seen.ended).toBe(0);
  });

  it("cancels the queue on Stop, and says nothing of an utterance that was cut", () => {
    vi.useFakeTimers();
    installVoice();
    const { seen, handlers: said } = handlers();
    const engine = deviceSpeechEngine();
    engine?.speak("One. Two.", said);
    vi.runAllTimers();
    engine?.stop();
    expect(cancelled).toBe(1);
    // the browser reports the cancelled utterance after the fact: it is not an answer that ended, nor a refusal
    spoken[0]?.onend?.();
    spoken[0]?.onerror?.({ error: "canceled" });
    expect(seen.ended).toBe(0);
    expect(seen.failed).toEqual([]);
  });

  it("says nothing at all when there is nothing to say", () => {
    installVoice();
    const { seen, handlers: said } = handlers();
    deviceSpeechEngine()?.speak("   ", said);
    expect(spoken).toEqual([]);
    expect(seen.ended).toBe(1);
  });
});
