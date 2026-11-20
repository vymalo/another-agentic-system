// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { SpeechHandlers } from "../lib/speech-engine";
import { resetSpeech, SPEECH_FAILED, setSpeechEngine, stopSpeechIf, useSpeech } from "./use-speech";

/*
 * One answer at a time, app-wide (ADR 0067 S1). The store is a module-level singleton, so every "Read aloud" on the page
 * reads the same one: starting another answer stops the one before it, Stop stops it, and the button goes back to "Read
 * aloud" when the voice ends or refuses. The browser's voice is a fake here (`setSpeechEngine`): what this holds is the
 * state machine, not the sound.
 *
 * The device is not the store: it can hold a queue the store no longer knows about (a chunk that failed half way), which
 * is why a press stops it before it speaks, and why a failure stops it too. `log` is what the fake was told, in order.
 */

class FakeEngine {
  spoken: string[] = [];
  log: string[] = [];
  stopped = 0;
  throwOnSpeak = false;
  private handlers: SpeechHandlers | null = null;
  speak(words: string, handlers: SpeechHandlers) {
    this.log.push(`speak:${words}`);
    if (this.throwOnSpeak) throw new Error("the voice refused at once");
    this.spoken.push(words);
    this.handlers = handlers;
  }
  stop() {
    this.log.push("stop");
    this.stopped += 1;
  }
  end() {
    this.handlers?.end();
  }
  fail(reason: string) {
    this.handlers?.error(reason);
  }
}

let engine: FakeEngine;

const use = () => {
  engine = new FakeEngine();
  setSpeechEngine(engine);
  return engine;
};

/** One answer's button, the way a turn draws it: Read aloud, then Stop while it is the answer being read. */
function Answer({ id, text }: { id: string; text: string }) {
  const { supported, speaking, failure, toggle } = useSpeech();
  if (!supported) return <p>no voice here</p>;
  return (
    <div>
      <button type="button" onClick={() => toggle(id, text)}>
        {speaking === id ? "Stop" : "Read aloud"}
      </button>
      <span data-slot="speech-note">{failure?.id === id ? failure.message : ""}</span>
    </div>
  );
}

const button = () => screen.getByRole("button", { name: /Read aloud|Stop/ });
const note = () => document.querySelector('[data-slot="speech-note"]')?.textContent ?? "";

afterEach(() => {
  cleanup();
  resetSpeech();
});

describe("useSpeech", () => {
  it("says the words and is Stop while this answer is read", () => {
    use();
    render(<Answer id="a" text="The words of the answer." />);
    fireEvent.click(button());
    expect(engine.spoken).toEqual(["The words of the answer."]);
    expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy();
  });

  it("stops when the person presses Stop, and says Read aloud again", () => {
    use();
    render(<Answer id="a" text="Words." />);
    fireEvent.click(button());
    fireEvent.click(button());
    expect(engine.stopped).toBe(2);
    expect(screen.getByRole("button", { name: "Read aloud" })).toBeTruthy();
  });

  it("reads one answer at a time: starting another stops the one before it", () => {
    use();
    render(
      <div>
        <Answer id="a" text="First answer." />
        <Answer id="b" text="Second answer." />
      </div>,
    );
    const [first, second] = screen.getAllByRole("button", { name: "Read aloud" });
    fireEvent.click(first as HTMLElement);
    expect(screen.getAllByRole("button", { name: "Stop" })).toHaveLength(1);
    engine.log = [];
    fireEvent.click(second as HTMLElement);
    // the first answer is cut before the second is queued
    expect(engine.log).toEqual(["stop", "speak:Second answer."]);
    expect(engine.spoken).toEqual(["First answer.", "Second answer."]);
    // the first answer is not read any more, the second is
    expect(screen.getAllByRole("button", { name: "Stop" })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "Read aloud" })).toHaveLength(1);
  });

  it("goes back to Read aloud when the answer ends", () => {
    use();
    render(<Answer id="a" text="Words." />);
    fireEvent.click(button());
    act(() => engine.end());
    expect(screen.getByRole("button", { name: "Read aloud" })).toBeTruthy();
  });

  it("says in words when the browser refuses, and does not throw", () => {
    use();
    render(<Answer id="a" text="Words." />);
    fireEvent.click(button());
    act(() => engine.fail("not-allowed"));
    expect(note()).toBe(SPEECH_FAILED);
    expect(screen.getByRole("button", { name: "Read aloud" })).toBeTruthy();
  });

  it("cuts the device's queue when a chunk fails, so the rest of the answer is not said", () => {
    use();
    render(<Answer id="a" text="Words." />);
    fireEvent.click(button());
    engine.log = [];
    act(() => engine.fail("synthesis-failed"));
    // the browser reports a failure at one chunk and does not always drop the chunks queued after it
    expect(engine.log).toEqual(["stop"]);
    expect(note()).toBe(SPEECH_FAILED);
  });

  it("stops the device before it speaks, even when it thinks nothing is read", () => {
    use();
    render(<Answer id="a" text="Words." />);
    fireEvent.click(button());
    act(() => engine.fail("synthesis-failed"));
    // a failed chunk can leave the device's own queue speaking while the store holds no answer
    engine.log = [];
    fireEvent.click(button());
    expect(engine.log).toEqual(["stop", "speak:Words."]);
  });

  it("says a voice that throws instead of reporting as a failure, and never throws", () => {
    const voice = use();
    voice.throwOnSpeak = true;
    render(<Answer id="a" text="Words." />);
    expect(() => fireEvent.click(button())).not.toThrow();
    expect(note()).toBe(SPEECH_FAILED);
    expect(screen.getByRole("button", { name: "Read aloud" })).toBeTruthy();
  });

  it("says the refusal on the answer it is about, and takes it back on the next attempt", () => {
    use();
    render(
      <div>
        <Answer id="a" text="First answer." />
        <Answer id="b" text="Second answer." />
      </div>,
    );
    const [first, second] = screen.getAllByRole("button", { name: "Read aloud" });
    fireEvent.click(first as HTMLElement);
    act(() => engine.fail("not-allowed"));
    expect(document.querySelectorAll('[data-slot="speech-note"]')[0]?.textContent).toBe(
      SPEECH_FAILED,
    );
    expect(document.querySelectorAll('[data-slot="speech-note"]')[1]?.textContent).toBe("");
    fireEvent.click(second as HTMLElement);
    expect(note()).toBe("");
  });

  it("ignores what the browser says after Stop (its cancelled utterance)", () => {
    use();
    render(<Answer id="a" text="Words." />);
    fireEvent.click(button());
    fireEvent.click(button());
    act(() => engine.end());
    act(() => engine.fail("canceled"));
    expect(screen.getByRole("button", { name: "Read aloud" })).toBeTruthy();
    expect(note()).toBe("");
  });

  it("says nothing when there is nothing to say", () => {
    use();
    render(<Answer id="a" text="   " />);
    fireEvent.click(button());
    expect(engine.spoken).toEqual([]);
    expect(screen.getByRole("button", { name: "Read aloud" })).toBeTruthy();
  });

  it("offers no voice where the browser has none", () => {
    setSpeechEngine(null);
    render(<Answer id="a" text="Words." />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("no voice here")).toBeTruthy();
  });

  it("stops an answer that leaves the page, and only that one", () => {
    use();
    render(
      <div>
        <Answer id="a" text="First answer." />
        <Answer id="b" text="Second answer." />
      </div>,
    );
    const [first, second] = screen.getAllByRole("button", { name: "Read aloud" });
    fireEvent.click(second as HTMLElement);
    engine.log = [];
    act(() => stopSpeechIf("a"));
    expect(engine.log).toEqual([]);
    expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy();
    act(() => stopSpeechIf("b"));
    expect(engine.log).toEqual(["stop"]);
    expect(screen.getAllByRole("button", { name: "Read aloud" })).toHaveLength(2);
    expect(first).toBeTruthy();
  });

  it("stops on Esc, and only while something is read", () => {
    use();
    render(<Answer id="a" text="Words." />);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(engine.stopped).toBe(0);
    fireEvent.click(button());
    fireEvent.keyDown(document, { key: "Escape" });
    expect(engine.stopped).toBe(2);
    expect(screen.getByRole("button", { name: "Read aloud" })).toBeTruthy();
    // and Esc is not taken when nothing is read: it belongs to whatever else is open
    fireEvent.keyDown(document, { key: "Escape" });
    expect(engine.stopped).toBe(2);
  });

  it("stops on Esc again after a failure, on the answer read next", () => {
    use();
    render(<Answer id="a" text="Words." />);
    fireEvent.click(button());
    act(() => engine.fail("synthesis-failed"));
    // the failure cut the queue, so there is nothing left for Esc to stop
    fireEvent.keyDown(document, { key: "Escape" });
    expect(engine.stopped).toBe(2);
    fireEvent.click(button());
    fireEvent.keyDown(document, { key: "Escape" });
    expect(engine.stopped).toBe(4);
    expect(screen.getByRole("button", { name: "Read aloud" })).toBeTruthy();
  });

  it("leaves Esc to a dialog that is open: it closes, and the answer goes on", () => {
    use();
    render(<Answer id="a" text="Words." />);
    fireEvent.click(button());
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    dialog.setAttribute("data-state", "open");
    document.body.append(dialog);
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(engine.stopped).toBe(1);
    expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy();
    dialog.remove();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(engine.stopped).toBe(2);
  });
});
