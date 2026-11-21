import { describe, expect, it } from "vitest";
import {
  deliveryNote,
  modeOfKey,
  readsWhen,
  STEER_URI,
  sendHint,
  stopHint,
  stopRoundHint,
} from "./send";

const key = (over: Partial<Parameters<typeof modeOfKey>[0]>) => ({
  key: "Enter",
  shiftKey: false,
  ctrlKey: false,
  metaKey: false,
  ...over,
});

describe("sending while an agent works", () => {
  it("names the extension of ADR 0036 by its exact URI", () => {
    expect(STEER_URI).toBe("https://agents.vymalo.com/a2a/extensions/steer/v1");
  });

  it("promises the next step only to an agent whose card lists steer/v1", () => {
    expect(readsWhen(true)).toBe("at its next step");
    expect(readsWhen(false)).toBe("after this turn");
    // a card that could not be read promises nothing
    expect(readsWhen(null)).toBe("after this turn");
    expect(sendHint("Coder", true)).toBe("Coder reads it at its next step");
    expect(sendHint("Reviewer", false)).toBe("Reviewer reads it after this turn");
    expect(stopHint("Coder")).toBe("Stops Coder and starts again with your message");
  });

  it("words the note of a message by how it was delivered", () => {
    expect(deliveryNote("steer", "Coder", true)).toBe(
      "Sent while Coder was working · read at its next step",
    );
    expect(deliveryNote("steer", "Coder", false)).toBe(
      "Sent while Coder was working · read after this turn",
    );
    expect(deliveryNote("interrupt", "Coder", true)).toBe(
      "Stopped Coder · it starts again from here",
    );
  });

  it("reads the keys: Enter sends, Ctrl or Cmd with Shift stops and sends, Shift+Enter is a new line", () => {
    expect(modeOfKey(key({}))).toBe("steer");
    expect(modeOfKey(key({ shiftKey: true, ctrlKey: true }))).toBe("interrupt");
    expect(modeOfKey(key({ shiftKey: true, metaKey: true }))).toBe("interrupt");
    expect(modeOfKey(key({ shiftKey: true }))).toBeNull();
    expect(modeOfKey(key({ ctrlKey: true }))).toBeNull();
    expect(modeOfKey(key({ altKey: true }))).toBeNull();
    expect(modeOfKey(key({ key: "a" }))).toBeNull();
  });

  it("has no Ctrl/⌘+Shift+Enter where Stop and send is not offered, and Enter still sends", () => {
    expect(modeOfKey(key({ shiftKey: true, ctrlKey: true }), false)).toBeNull();
    expect(modeOfKey(key({ shiftKey: true, metaKey: true }), false)).toBeNull();
    expect(modeOfKey(key({}), false)).toBe("steer");
    expect(modeOfKey(key({ shiftKey: true, ctrlKey: true }), true)).toBe("interrupt");
  });

  it("says what Stop does to a subagent: it stays open, or it closes with its round", () => {
    expect(stopRoundHint(true)).toBe("Stops this round, the subagent stays open");
    expect(stopRoundHint(false)).toBe("Stops this round and closes the subagent");
  });
});
