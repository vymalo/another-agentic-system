import { describe, expect, it } from "vitest";
import { type Prior, regenerableFrom } from "./regenerate";

/*
 * What begins a turn is a message of the person (a `user` message the log gave a `seq`): the run an
 * agent answers it with is the assistant message right after it. A turn that began with a form
 * answer has no such message in front of it (the answer is an action, in the agent's own message).
 */

const said = (over: Partial<Prior> = {}): Prior => ({
  role: "user",
  content: [{ type: "text", text: "echo first" }],
  metadata: {},
  ...over,
});
const ok = { seq: 3, mentioned: false, waiting: false };

describe("regenerableFrom", () => {
  it("is the person's words and their place in the log", () => {
    expect(regenerableFrom({ prior: said(), ...ok })).toEqual({ seq: 3, text: "echo first" });
  });

  it("joins the text parts and ignores the others", () => {
    const prior = said({
      content: [{ type: "text", text: "a" }, { type: "image" }, { type: "text", text: "b" }],
    });
    expect(regenerableFrom({ prior, ...ok })?.text).toBe("a\n\nb");
  });

  it("is nothing when the message before the turn is not the person's (a form answer began it)", () => {
    expect(regenerableFrom({ prior: said({ role: "assistant" }), ...ok })).toBeUndefined();
    expect(regenerableFrom({ prior: undefined, ...ok })).toBeUndefined();
  });

  it("is nothing before the message's place in the log is known", () => {
    expect(regenerableFrom({ prior: said(), ...ok, seq: undefined })).toBeUndefined();
  });

  it("is nothing for words that are empty", () => {
    expect(regenerableFrom({ prior: said({ content: [] }), ...ok })).toBeUndefined();
  });

  it("is nothing when the message mentioned an agent: an edit carries no mentions", () => {
    expect(regenerableFrom({ prior: said(), ...ok, mentioned: true })).toBeUndefined();
  });

  it("is nothing while a question waits for an answer: the edit would drop it", () => {
    expect(regenerableFrom({ prior: said(), ...ok, waiting: true })).toBeUndefined();
  });

  it("is nothing for a message sent while the agent worked (steer or interrupt): the turn is more than it", () => {
    for (const delivery of ["steer", "interrupt"]) {
      const prior = said({ metadata: { custom: { delivery } } });
      expect(regenerableFrom({ prior, ...ok })).toBeUndefined();
    }
  });
});
