import { describe, expect, it } from "vitest";
import type { ApiAgent } from "@/lib/api/types";
import {
  forTrimmed,
  insertMention,
  insidePair,
  labelOf,
  MAX_MENTIONS,
  type Mention,
  matching,
  mentionable,
  parseMentions,
  reconcile,
  segments,
  standing,
  stands,
  triggerAt,
  withoutMention,
} from "./mentions";

const agent = (id: string, name = id, extra: Partial<ApiAgent> = {}): ApiAgent => ({
  id,
  name,
  ...extra,
});

/** A mention of `id` found where `label` first stands in `text`, as the composer would have written it. */
const at = (text: string, id: string, from = 0): Mention => {
  const label = labelOf(id);
  const start = text.indexOf(label, from);
  if (start < 0) throw new Error(`${label} is not in ${JSON.stringify(text)}`);
  return { agentId: id, label, start, end: start + label.length };
};

/** What the orchestrator checks of a reference: the label is the text at its offsets. */
const consistent = (text: string, mentions: readonly Mention[]) => {
  for (const m of mentions) expect(text.slice(m.start, m.end)).toBe(m.label);
};

describe("labels and offsets count UTF-16 code units", () => {
  it("an emoji is two code units, so a mention after one starts two later than in code points", () => {
    const text = "😄 ask @researcher";
    const m = at(text, "researcher");
    // "😄" is two units, a space, "ask", a space: 2 + 1 + 3 + 1
    expect(m.start).toBe(7);
    expect([...text.slice(0, m.start)].length).toBe(6); // in code points it would be one less
    expect(stands(text, m)).toBe(true);
  });

  it("an offset between the two halves of a surrogate pair is no mention", () => {
    const text = "😄@coder";
    expect(insidePair(text, 1)).toBe(true);
    expect(insidePair(text, 0)).toBe(false);
    expect(insidePair(text, 2)).toBe(false);
    expect(stands(text, { agentId: "coder", label: "@coder", start: 2, end: 8 })).toBe(false); // no space before it
    expect(stands("😄 @coder", { agentId: "coder", label: "@coder", start: 3, end: 9 })).toBe(true);
    expect(stands("😄 @coder", { agentId: "coder", label: "@coder", start: 1, end: 7 })).toBe(
      false,
    );
  });

  it("the label of an agent id is at most 64 units", () => {
    expect(mentionable("a".repeat(63))).toBe(true);
    expect(mentionable("a".repeat(64))).toBe(false);
  });
});

describe("reconcile: a mention follows the text it stands in", () => {
  it("typing in front of it moves it by what was typed", () => {
    const before = "ask @coder now";
    const after = "please ask @coder now";
    const [m] = reconcile(before, after, [at(before, "coder")]);
    expect(m).toMatchObject({ start: 11, end: 17 });
    consistent(after, [m as Mention]);
  });

  it("an emoji or a letter with a combining mark typed in front moves it by two units each", () => {
    const before = "see @coder";
    const emoji = reconcile(before, "😄 see @coder", [at(before, "coder")]);
    // "😄" is two code units and a space is one: 4 + 3
    expect(emoji).toEqual([{ agentId: "coder", label: "@coder", start: 7, end: 13 }]);
    consistent("😄 see @coder", emoji);
    // "e" and U+0301 (a combining acute accent) are one letter to the eye and two code units, as many as the emoji
    const accent = `${String.fromCharCode(0x65, 0x301)} see @coder`;
    expect(accent.length).toBe(13);
    expect([...accent].length).toBe(13); // two code points as well: only the grapheme is one
    const moved = reconcile(before, accent, [at(before, "coder")]);
    expect(moved[0]?.start).toBe(7);
    consistent(accent, moved);
  });

  it("text after it, and an edit at its far end, leave it where it is", () => {
    const before = "ask @coder";
    const after = "ask @coder to plot it";
    expect(reconcile(before, after, [at(before, "coder")])).toEqual([at(after, "coder")]);
  });

  it("deleting in front moves it back; a paste over a selection in front moves it by the difference", () => {
    const before = "hello world @coder";
    expect(reconcile(before, "hi @coder", [at(before, "coder")])).toEqual([
      at("hi @coder", "coder"),
    ]);
    expect(reconcile(before, "😄😄😄 @coder", [at(before, "coder")])).toEqual([
      at("😄😄😄 @coder", "coder"),
    ]);
  });

  it("an edit inside the label drops the mention, and so does one that changes a letter of it", () => {
    const before = "ask @researcher";
    const m = at(before, "researcher");
    expect(reconcile(before, "ask @resarcher", [m])).toEqual([]);
    expect(reconcile(before, "ask @researcher2", [m])).toEqual([]); // it is another word now
    expect(reconcile(before, "ask @researche", [m])).toEqual([]);
    expect(reconcile(before, "ask @Researcher", [m])).toEqual([]);
    expect(reconcile(before, "ask researcher", [m])).toEqual([]);
  });

  it("a word glued to the front makes another word, so the mention goes; punctuation after it does not", () => {
    const before = "ask @coder";
    const m = at(before, "coder");
    expect(reconcile(before, "ask x@coder", [m])).toEqual([]);
    expect(reconcile(before, "ask @coder,", [m])).toEqual([at("ask @coder,", "coder")]);
    expect(reconcile(before, "ask @coder.", [m])).toHaveLength(1);
    expect(reconcile(before, "ask @coder-x", [m])).toEqual([]);
  });

  it("white space added or cut at the ends (the runtime trims what it sends) moves the mentions with the front", () => {
    const before = "  ask @coder \n";
    const m = at(before, "coder");
    expect(reconcile(before, "ask @coder", [m])).toEqual([at("ask @coder", "coder")]);
    expect(reconcile("ask @coder", before, [at("ask @coder", "coder")])).toEqual([m]);
  });

  it("emptying the text drops every mention", () => {
    const before = "@a and @b";
    expect(reconcile(before, "", [at(before, "a"), at(before, "b")])).toEqual([]);
  });

  it("two mentions: deleting one keeps the other, moved", () => {
    const before = "@a then @b";
    const mentions = [at(before, "a"), at(before, "b")];
    const after = "then @b";
    expect(reconcile(before, after, mentions)).toEqual([at(after, "b")]);
    const rest = "@a then";
    expect(reconcile(before, rest, mentions)).toEqual([at(rest, "a")]);
  });

  it("the same agent twice: removing the first leaves one, whichever way the edit is read", () => {
    const before = "@a x @a y";
    const mentions = [at(before, "a"), at(before, "a", 3)];
    const after = "x @a y";
    const kept = reconcile(before, after, mentions);
    expect(kept).toHaveLength(1);
    consistent(after, kept);
  });

  it("an edit that replaces one half of a surrogate pair never splits the pair", () => {
    const before = "😄 @coder";
    // the first emoji becomes another one: the high surrogate is shared (U+D83D), only the low half differs
    const after = "😀 @coder";
    expect("😄".charCodeAt(0)).toBe("😀".charCodeAt(0));
    const kept = reconcile(before, after, [at(before, "coder")]);
    expect(kept).toEqual([at(after, "coder")]);
    // and an emoji put in front of one that shares its high half
    const more = reconcile(before, "😀😄 @coder", [at(before, "coder")]);
    expect(more).toEqual([at("😀😄 @coder", "coder")]);
  });

  it("combining marks next to a label: a mark glued after it drops it only when it is a handle character", () => {
    const before = "ask @coder";
    const m = at(before, "coder");
    // a combining acute accent is not a handle character, but it changes the last letter to the eye;
    // the offsets still hold, so the orchestrator would accept it: it stays
    const after = "ask @codeŕ";
    expect(reconcile(before, after, [m])).toEqual([at(after, "coder")]);
  });

  it("the result is sorted and every mention stands", () => {
    const before = "@b @a";
    const after = "x @b @a";
    const kept = reconcile(before, after, [at(before, "a"), at(before, "b")]);
    expect(kept.map((m) => m.agentId)).toEqual(["b", "a"]);
    for (const m of kept) expect(stands(after, m)).toBe(true);
  });
});

describe("forTrimmed and standing: what a message goes out with", () => {
  it("moves the mentions past the white space cut off the front", () => {
    const text = "  \n  ask @coder";
    const out = forTrimmed(text, [at(text, "coder")]);
    expect(out).toEqual([{ agentId: "coder", label: "@coder", start: 4, end: 10 }]);
    consistent(text.trim(), out);
  });

  it("with an emoji before the mention", () => {
    const text = " 😄 ask @coder 👍";
    const out = forTrimmed(text, [at(text, "coder")]);
    consistent(text.trim(), out);
    expect(out[0]?.start).toBe("😄 ask ".length);
  });

  it("drops overlaps and what no longer stands, keeps the first overlapping one, and caps the count", () => {
    const text = "@a @a";
    const a = at(text, "a");
    const clash: Mention = { agentId: "b", label: "@a", start: 0, end: 2 };
    expect(standing(text, [a, clash])).toEqual([a]);
    const many = Array.from({ length: 20 }, (_, i) => `@a${i}`).join(" ");
    const all = Array.from({ length: 20 }, (_, i) => at(many, `a${i}`));
    expect(standing(many, all)).toHaveLength(MAX_MENTIONS);
  });

  it("a mention whose label is not the text at its offsets stands for nothing", () => {
    expect(
      standing("ask @coder", [{ agentId: "coder", label: "@coder", start: 0, end: 6 }]),
    ).toEqual([]);
  });
});

describe("triggerAt: the mention being typed", () => {
  it("an @ that begins a word, up to the caret", () => {
    expect(triggerAt("ask @res", 8)).toEqual({ start: 4, query: "res" });
    expect(triggerAt("@", 1)).toEqual({ start: 0, query: "" });
    expect(triggerAt("a\n@c", 4)).toEqual({ start: 2, query: "c" });
  });

  it("is not one inside a word, an e-mail address, or after the caret has moved on", () => {
    expect(triggerAt("mail me@example.com", 19)).toBeNull();
    expect(triggerAt("ask @res now", 12)).toBeNull(); // the caret is past a space
    expect(triggerAt("ask @a@b", 8)).toBeNull();
    expect(triggerAt("hello", 5)).toBeNull();
  });

  it("works after an emoji: the offsets are code units", () => {
    expect(triggerAt("😄 @co", 6)).toEqual({ start: 3, query: "co" });
    expect(triggerAt("😄@co", 5)).toBeNull();
  });

  it("reads up to the caret, not to the end of the word", () => {
    expect(triggerAt("ask @researcher", 7)).toEqual({ start: 4, query: "re" });
  });
});

describe("matching", () => {
  const list = [
    agent("coder", "Coder"),
    agent("researcher", "Mock researcher"),
    agent("mock-browser", "Browser"),
  ];
  it("by the start of the id, of the name or of a word in either", () => {
    expect(matching(list, "").map((a) => a.id)).toEqual(["coder", "researcher", "mock-browser"]);
    expect(matching(list, "co").map((a) => a.id)).toEqual(["coder"]);
    expect(matching(list, "res").map((a) => a.id)).toEqual(["researcher"]);
    expect(matching(list, "bro").map((a) => a.id)).toEqual(["mock-browser"]);
    expect(matching(list, "MOCK").map((a) => a.id)).toEqual(["researcher", "mock-browser"]);
    expect(matching(list, "zzz")).toEqual([]);
  });
});

describe("insertMention and withoutMention", () => {
  const coder = agent("coder", "Coder", { cardUrl: "http://coder/card" });

  it("replaces the typed word with the label and a space, and says where it is", () => {
    const text = "ask @co";
    const trigger = triggerAt(text, 7);
    if (!trigger) throw new Error("no trigger");
    const out = insertMention(text, trigger, 7, coder);
    expect(out.text).toBe("ask @coder ");
    expect(out.caret).toBe(11);
    expect(out.mention).toEqual({
      agentId: "coder",
      label: "@coder",
      start: 4,
      end: 10,
      cardUrl: "http://coder/card",
    });
    consistent(out.text, [out.mention]);
  });

  it("adds no space when one follows, and keeps the text after the caret", () => {
    const text = "ask @co to plot";
    const trigger = triggerAt(text, 7);
    if (!trigger) throw new Error("no trigger");
    const out = insertMention(text, trigger, 7, coder);
    expect(out.text).toBe("ask @coder to plot");
    expect(out.caret).toBe(10);
  });

  it("after an emoji the offsets are in code units", () => {
    const text = "😄 @co";
    const trigger = triggerAt(text, 6);
    if (!trigger) throw new Error("no trigger");
    const out = insertMention(text, trigger, 6, coder);
    expect(out.mention.start).toBe(3);
    consistent(out.text, [out.mention]);
  });

  it("an agent without a card URL gives a reference without one", () => {
    const trigger = triggerAt("@x", 2);
    if (!trigger) throw new Error("no trigger");
    expect("cardUrl" in insertMention("@x", trigger, 2, agent("x")).mention).toBe(false);
  });

  it("existing mentions move with the pick (reconcile of the two texts)", () => {
    const text = "@a then @c";
    const first = at(text, "a");
    const trigger = triggerAt(text, 10);
    if (!trigger) throw new Error("no trigger");
    const out = insertMention(text, trigger, 10, coder);
    expect(reconcile(text, out.text, [first])).toEqual([first]);
    // and a pick in front of an existing mention moves it
    const later = "@c rest @a";
    const t2 = triggerAt(later.slice(0, 2), 2);
    if (!t2) throw new Error("no trigger");
    const out2 = insertMention(later, t2, 2, coder);
    expect(out2.text).toBe("@coder  rest @a".replace("@coder  ", "@coder "));
    const moved = reconcile(later, out2.text, [at(later, "a")]);
    consistent(out2.text, moved);
  });

  it("withoutMention takes the label and the space after it out", () => {
    const text = "ask @coder to plot";
    expect(withoutMention(text, at(text, "coder"))).toBe("ask to plot");
    const end = "ask @coder";
    expect(withoutMention(end, at(end, "coder"))).toBe("ask ");
  });
});

describe("parseMentions and segments: what the log carries, drawn", () => {
  const text = "ask @researcher then @coder";
  const refs = [at(text, "researcher"), at(text, "coder")];

  it("reads references of the right shape and sorts them", () => {
    expect(parseMentions([...refs].reverse(), text)).toEqual(refs);
    expect(parseMentions(null)).toEqual([]);
    expect(
      parseMentions([{ agentId: 1 }, "x", null, { agentId: "a", label: "@a", start: -1, end: 2 }]),
    ).toEqual([]);
  });

  it("keeps a card URL, and drops a reference the text does not bear out", () => {
    const withCard = { ...refs[0], cardUrl: "http://r/card" };
    expect(parseMentions([withCard], text)).toEqual([withCard]);
    expect(parseMentions([{ ...refs[0], start: 1, end: 12 }], text)).toEqual([]);
  });

  it("cuts the text at its mentions", () => {
    expect(segments(text, refs)).toEqual([
      { text: "ask " },
      { text: "@researcher", mention: refs[0] },
      { text: " then " },
      { text: "@coder", mention: refs[1] },
    ]);
    expect(segments("plain", [])).toEqual([{ text: "plain" }]);
  });

  it("with an emoji in front the offsets still cut the right words", () => {
    const t = "😄😄 @coder 😄";
    const m = at(t, "coder");
    expect(segments(t, [m]).map((s) => s.text)).toEqual(["😄😄 ", "@coder", " 😄"]);
  });
});
