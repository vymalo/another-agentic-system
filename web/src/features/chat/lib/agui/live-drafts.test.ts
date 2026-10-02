import { describe, expect, it } from "vitest";
import {
  applyLive,
  type Draft,
  drawnDrafts,
  isLiveNow,
  type LiveEvent,
  liveMark,
  MAX_DRAFT_UNITS,
  MAX_DRAFTS,
  pending,
  resolveGroup,
} from "./live-drafts";
import { loadGolden } from "./testing";

const ACTOR = { type: "agent", name: "plain" };

const start = (id = "msg-3", extra: Record<string, unknown> = {}): LiveEvent => ({
  type: "TEXT_MESSAGE_START",
  messageId: id,
  role: "assistant",
  name: "plain",
  subagentRunId: "sub-2",
  metadata: { "vymalo.actor": ACTOR, "vymalo.live": {} },
  ...extra,
});
const content = (offset: number, delta: string, id = "msg-3"): LiveEvent => ({
  type: "TEXT_MESSAGE_CONTENT",
  messageId: id,
  delta,
  subagentRunId: "sub-2",
  metadata: { "vymalo.live": { offset } },
});
const finalContent = (offset: number, delta: string, id = "msg-3"): LiveEvent => ({
  type: "TEXT_MESSAGE_CONTENT",
  messageId: id,
  delta,
  subagentRunId: "sub-2",
  metadata: { "vymalo.live": { offset, final: true } },
});
const finalEnd = (id = "msg-3"): LiveEvent => ({
  type: "TEXT_MESSAGE_END",
  messageId: id,
  subagentRunId: "sub-2",
  metadata: { "vymalo.live": { final: true } },
});
/** The END of the log's message for a draft that turned out to be working text (ADR 0031). */
const workingEnd = (id = "msg-3"): LiveEvent => ({
  type: "TEXT_MESSAGE_END",
  messageId: id,
  subagentRunId: "sub-2",
  metadata: { "vymalo.live": { final: true, purpose: "working" } },
});
const abandoned = (id = "msg-3"): LiveEvent => ({
  type: "TEXT_MESSAGE_END",
  messageId: id,
  subagentRunId: "sub-2",
  metadata: { "vymalo.live": { abandoned: true } },
});

const fold = (events: LiveEvent[], from: readonly Draft[] = []): readonly Draft[] =>
  events.reduce(applyLive, from);

const drafted = (text: string, extra: Partial<Draft> = {}): Draft => ({
  id: "msg-3",
  text,
  name: "plain",
  subagentRunId: "sub-2",
  actor: ACTOR as Draft["actor"],
  ...extra,
});

describe("liveMark: which frames are live", () => {
  it("reads the mark of a text frame and nothing else", () => {
    expect(liveMark(start())).toEqual({ offset: undefined, final: false, abandoned: false });
    expect(liveMark(content(3, "x"))).toEqual({ offset: 3, final: false, abandoned: false });
    expect(liveMark(finalContent(18, ""))).toEqual({ offset: 18, final: true, abandoned: false });
    expect(liveMark(abandoned())).toEqual({ offset: undefined, final: false, abandoned: true });
    // an ordinary frame, a frame of another type, a mark that is not an object
    expect(liveMark({ type: "TEXT_MESSAGE_CONTENT", messageId: "a", delta: "x" })).toBeNull();
    expect(liveMark({ type: "RUN_STARTED", metadata: { "vymalo.live": {} } })).toBeNull();
    expect(liveMark({ type: "TEXT_MESSAGE_START", metadata: { "vymalo.live": "yes" } })).toBeNull();
  });

  it("an offset that is not a whole count is no offset, and only `true` is final or abandoned", () => {
    for (const offset of [-1, 1.5, "3", null, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(liveMark(content(offset as number, "x"))?.offset).toBeUndefined();
    }
    const odd: LiveEvent = {
      type: "TEXT_MESSAGE_END",
      messageId: "a",
      metadata: { "vymalo.live": { final: "true", abandoned: 1 } },
    };
    expect(liveMark(odd)).toEqual({ offset: undefined, final: false, abandoned: false });
  });

  it("the log's own final frames travel in their group; every other live frame is handled at once", () => {
    expect(isLiveNow(start())).toBe(true);
    expect(isLiveNow(content(0, "x"))).toBe(true);
    expect(isLiveNow(abandoned())).toBe(true);
    expect(isLiveNow(finalContent(3, "x"))).toBe(false);
    expect(isLiveNow(finalEnd())).toBe(false);
    expect(isLiveNow({ type: "TEXT_MESSAGE_START", messageId: "a", role: "user" })).toBe(false);
  });
});

describe("applyLive: the words as they are written", () => {
  it("a START opens an empty draft that knows who writes it", () => {
    const drafts = fold([start()]);
    expect(drafts).toEqual([drafted("")]);
  });

  it("the pieces of the golden grow the draft: Fib, onacci , in Rust.", () => {
    const pieces = loadGolden("stream")
      .map((f) => f.event as LiveEvent)
      .filter(isLiveNow);
    expect(pieces).toHaveLength(4); // START and three pieces; the final frames are the log's own
    const drafts = fold(pieces);
    expect(drafts).toMatchObject([
      { id: "msg-3", text: "Fibonacci in Rust.", name: "plain", subagentRunId: "sub-2" },
    ]);
    expect(drafts[0]?.actor).toEqual({ type: "agent", name: "plain" });
  });

  it("a piece that overlaps what is held grows it by the part beyond", () => {
    expect(fold([start(), content(0, "Fibonacci"), content(3, "onacci in")])[0]?.text).toBe(
      "Fibonacci in",
    );
  });

  it("a repeat says nothing: the same array comes back", () => {
    const held = fold([start(), content(0, "Fibonacci")]);
    expect(applyLive(held, content(0, "Fib"))).toBe(held);
    expect(applyLive(held, content(0, "Fibonacci"))).toBe(held);
    expect(applyLive(held, content(9, ""))).toBe(held);
  });

  it("a gap is ignored until the text is said again from the start", () => {
    const held = fold([start(), content(0, "Fib")]);
    expect(applyLive(held, content(10, "in Rust."))).toBe(held);
    // the sender's refresh, from offset 0
    expect(applyLive(held, content(0, "Fibonacci in Rust."))[0]?.text).toBe("Fibonacci in Rust.");
  });

  it("offsets count UTF-16 code units: an emoji is two", () => {
    const first = "hi \u{1F43C}"; // 3 + 2 units
    expect(first.length).toBe(5);
    const held = fold([start(), content(0, first)]);
    expect(applyLive(held, content(5, " there"))[0]?.text).toBe("hi \u{1F43C} there");
    // a byte or code-point offset (4) would land inside the pair or repeat it: nothing changes the held text wrongly
    expect(applyLive(held, content(6, "x"))).toBe(held);
  });

  it("a piece for a draft that is not open, or without a usable offset, says nothing", () => {
    const held = fold([start(), content(0, "Fib")]);
    expect(applyLive(held, content(3, "x", "other"))).toBe(held);
    expect(applyLive(held, { ...content(3, "x"), metadata: { "vymalo.live": {} } })).toBe(held);
    expect(applyLive([], content(0, "x"))).toEqual([]);
    expect(applyLive(held, { ...start(), messageId: undefined })).toBe(held);
  });

  it("an abandoned END removes its draft and only its draft", () => {
    const held = fold([start("a"), content(0, "one", "a"), start("b"), content(0, "two", "b")]);
    expect(held.map((d) => d.id)).toEqual(["a", "b"]);
    expect(applyLive(held, abandoned("a")).map((d) => d.id)).toEqual(["b"]);
    expect(applyLive(held, abandoned("zzz"))).toBe(held);
  });

  it("a START again for an id begins that draft over", () => {
    const held = fold([start(), content(0, "Fib")]);
    expect(fold([start()], held)).toEqual([drafted("")]);
  });

  it("holds at most MAX_DRAFTS drafts, the newest", () => {
    const events = Array.from({ length: MAX_DRAFTS + 2 }, (_, i) => start(`m-${i}`));
    const held = fold(events);
    expect(held).toHaveLength(MAX_DRAFTS);
    expect(held[0]?.id).toBe("m-2");
    expect(held.at(-1)?.id).toBe(`m-${MAX_DRAFTS + 1}`);
  });

  it("a draft stops growing at its bound", () => {
    const full = "x".repeat(MAX_DRAFT_UNITS);
    const held = fold([start(), content(0, full)]);
    expect(held[0]?.text).toHaveLength(MAX_DRAFT_UNITS);
    expect(applyLive(held, content(MAX_DRAFT_UNITS, "y"))).toBe(held);
    expect(fold([start(), content(0, `${full}y`)])[0]?.text).toBe("");
  });

  it("a frame that is the log's own, or no live frame, changes nothing", () => {
    const held = fold([start(), content(0, "Fib")]);
    expect(applyLive(held, finalContent(3, "onacci"))).toBe(held);
    expect(applyLive(held, finalEnd())).toBe(held);
    expect(applyLive(held, { type: "RUN_FINISHED" })).toBe(held);
  });
});

describe("resolveGroup: the log's message takes the draft over", () => {
  it("a group with no live final is handed back as it is", () => {
    const events: LiveEvent[] = [
      { type: "TEXT_MESSAGE_START", messageId: "x", role: "assistant" },
      { type: "TEXT_MESSAGE_CONTENT", messageId: "x", delta: "hi" },
      { type: "TEXT_MESSAGE_END", messageId: "x" },
    ];
    const drafts = [drafted("Fib")];
    const out = resolveGroup(drafts, events);
    expect(out?.events).toBe(events);
    expect(out?.drafts).toBe(drafts);
  });

  it("CONTENT{offset, final} + END{final} become the plain message: the draft up to the offset plus the rest", () => {
    const out = resolveGroup([drafted("Fibonacci in Rust.")], [finalContent(18, ""), finalEnd()]);
    expect(out?.events).toEqual([
      {
        type: "TEXT_MESSAGE_START",
        messageId: "msg-3",
        role: "assistant",
        name: "plain",
        subagentRunId: "sub-2",
        metadata: { "vymalo.actor": ACTOR },
      },
      {
        type: "TEXT_MESSAGE_CONTENT",
        messageId: "msg-3",
        delta: "Fibonacci in Rust.",
        subagentRunId: "sub-2",
      },
      { type: "TEXT_MESSAGE_END", messageId: "msg-3", subagentRunId: "sub-2" },
    ]);
    // no live mark is left on what the runtime gets
    expect(out?.events.some((e) => liveMark(e) !== null)).toBe(false);
    // the draft knows the whole text, to be hidden when the transcript shows it
    expect(out?.drafts).toEqual([drafted("Fibonacci in Rust.", { final: "Fibonacci in Rust." })]);
  });

  it("the golden's group (seq 3) completes the golden's draft", () => {
    const frames = loadGolden("stream").map((f) => f.event as LiveEvent);
    const drafts = fold(frames.filter(isLiveNow));
    const group = frames.filter((e) => liveMark(e)?.final);
    expect(group).toHaveLength(2);
    const out = resolveGroup(drafts, group);
    expect(out?.events.map((e) => e.type)).toEqual([
      "TEXT_MESSAGE_START",
      "TEXT_MESSAGE_CONTENT",
      "TEXT_MESSAGE_END",
    ]);
    expect(out?.events[1]?.delta).toBe("Fibonacci in Rust.");
  });

  it("the final says only the rest: the draft holds the start of the text", () => {
    const out = resolveGroup([drafted("Fibonacci")], [finalContent(9, " in Rust."), finalEnd()]);
    expect(out?.events[1]?.delta).toBe("Fibonacci in Rust.");
  });

  it("a final that replaced what was said (offset 0) is the whole text, whatever the draft held", () => {
    const out = resolveGroup(
      [drafted("Fibbonacci")],
      [finalContent(0, "Fibonacci in Rust."), finalEnd()],
    );
    expect(out?.events[1]?.delta).toBe("Fibonacci in Rust.");
  });

  it("offset 0 needs no draft: the frame is the whole text", () => {
    const out = resolveGroup([], [finalContent(0, "All of it"), finalEnd()]);
    expect(out?.events.map((e) => e.type)).toEqual([
      "TEXT_MESSAGE_START",
      "TEXT_MESSAGE_CONTENT",
      "TEXT_MESSAGE_END",
    ]);
    expect(out?.events[0]).toMatchObject({ messageId: "msg-3", subagentRunId: "sub-2" });
    expect(out?.events[1]?.delta).toBe("All of it");
  });

  it("a final that continues text this connection never held cannot be told whole", () => {
    expect(resolveGroup([], [finalContent(18, ""), finalEnd()])).toBeNull();
    expect(resolveGroup([drafted("Fib")], [finalContent(18, ""), finalEnd()])).toBeNull();
    expect(
      resolveGroup([drafted("Fib", { id: "other" })], [finalContent(3, "x"), finalEnd()]),
    ).toBeNull();
  });

  it("an END of the log's message with no CONTENT before it, or a final without an offset, cannot be told whole", () => {
    expect(resolveGroup([drafted("Fib")], [finalEnd()])).toBeNull();
    const noOffset: LiveEvent = {
      type: "TEXT_MESSAGE_CONTENT",
      messageId: "msg-3",
      delta: "x",
      metadata: { "vymalo.live": { final: true } },
    };
    expect(resolveGroup([drafted("Fib")], [noOffset, finalEnd()])).toBeNull();
  });

  it("the other frames of the group keep their place around the message", () => {
    const before: LiveEvent = { type: "ACTIVITY_SNAPSHOT", messageId: "evt-3", activityType: "x" };
    const after: LiveEvent = { type: "SUBAGENT_FINISHED", subagentRunId: "sub-2" };
    const out = resolveGroup([drafted("Hi")], [before, finalContent(2, "!"), finalEnd(), after]);
    expect(out?.events.map((e) => e.type)).toEqual([
      "ACTIVITY_SNAPSHOT",
      "TEXT_MESSAGE_START",
      "TEXT_MESSAGE_CONTENT",
      "TEXT_MESSAGE_END",
      "SUBAGENT_FINISHED",
    ]);
  });

  it("only the draft the group completes is marked; another stays as it is", () => {
    const other = drafted("later", { id: "msg-4" });
    const out = resolveGroup([drafted("Hi"), other], [finalContent(2, ""), finalEnd()]);
    expect(out?.drafts[0]?.final).toBe("Hi");
    expect(out?.drafts[1]).toBe(other);
  });

  it("slices by UTF-16 units: a final after an emoji", () => {
    const out = resolveGroup([drafted("hi \u{1F43C}")], [finalContent(5, " there"), finalEnd()]);
    expect(out?.events[1]?.delta).toBe("hi \u{1F43C} there");
  });
});

describe("pending and drawnDrafts: the swap to the log's message", () => {
  it("pending drops the drafts the log completed, and returns the same array when there are none", () => {
    const open = drafted("Fib");
    const done = drafted("Fibonacci", { id: "msg-4", final: "Fibonacci" });
    expect(pending([open, done])).toEqual([open]);
    const only = [open];
    expect(pending(only)).toBe(only);
  });

  it("an open draft draws its text, with the name of the agent", () => {
    expect(drawnDrafts([drafted("Fib")], [])).toEqual([
      { id: "msg-3", text: "Fib", name: "plain" },
    ]);
  });

  it("an empty draft draws nothing", () => {
    expect(drawnDrafts([drafted("")], [])).toEqual([]);
    expect(drawnDrafts([drafted("  \n")], [])).toEqual([]);
  });

  it("a completed draft says the log's words until the transcript has them, then nothing", () => {
    const done = drafted("Fib", { final: "Fibonacci in Rust." });
    expect(drawnDrafts([done], ["some other words"])).toEqual([
      { id: "msg-3", text: "Fibonacci in Rust.", name: "plain" },
    ]);
    expect(drawnDrafts([done], ["Fibonacci in Rust."])).toEqual([]);
    // the transcript may join the words of two messages in one text part
    expect(drawnDrafts([done], ["Plan: write it.Fibonacci in Rust."])).toEqual([]);
  });
});

describe("working text (ADR 0031): a draft that turns out not to be the answer", () => {
  it("reads the purpose an END says, and only `working` or `answer`", () => {
    expect(liveMark(workingEnd())).toEqual({
      offset: undefined,
      final: true,
      abandoned: false,
      purpose: "working",
    });
    expect(liveMark(finalEnd())?.purpose).toBeUndefined();
    const odd: LiveEvent = {
      type: "TEXT_MESSAGE_END",
      messageId: "a",
      metadata: { "vymalo.live": { final: true, purpose: "thinking" } },
    };
    expect(liveMark(odd)?.purpose).toBeUndefined();
  });

  it("the plain message the runtime reads says it on its START, and the draft remembers it", () => {
    const out = resolveGroup([drafted("Let me look.")], [finalContent(12, ""), workingEnd()]);
    expect(out?.events[0]).toMatchObject({
      type: "TEXT_MESSAGE_START",
      messageId: "msg-3",
      metadata: { "vymalo.actor": ACTOR, "vymalo.purpose": "working" },
    });
    expect(out?.events.some((e) => liveMark(e) !== null)).toBe(false);
    expect(out?.drafts).toEqual([
      drafted("Let me look.", { final: "Let me look.", purpose: "working" }),
    ]);
  });

  it("says it even for a message with no draft to continue (a final that is the whole text)", () => {
    const out = resolveGroup([], [finalContent(0, "Whole."), workingEnd()]);
    expect(out?.events[0]).toMatchObject({ metadata: { "vymalo.purpose": "working" } });
  });

  it("an END that says only `final` is the answer: the START has no purpose and the draft stays to be drawn", () => {
    const out = resolveGroup([drafted("The answer.")], [finalContent(11, ""), finalEnd()]);
    expect(out?.events[0]).toMatchObject({ metadata: { "vymalo.actor": ACTOR } });
    expect(
      (out?.events[0]?.metadata as Record<string, unknown>)?.["vymalo.purpose"],
    ).toBeUndefined();
    expect(out?.drafts[0]?.purpose).toBeUndefined();
    expect(drawnDrafts(out?.drafts ?? [], [])).toEqual([
      { id: "msg-3", text: "The answer.", name: "plain" },
    ]);
  });

  it("a working draft draws nothing in the conversation, not even before the transcript has its words", () => {
    const done = drafted("Let me look.", { final: "Let me look.", purpose: "working" });
    expect(drawnDrafts([done], [])).toEqual([]);
    expect(drawnDrafts([done], ["Let me look."])).toEqual([]);
    // another draft of the same turn is not affected
    const open = drafted("Now the answer", { id: "msg-5" });
    expect(drawnDrafts([done, open], [])).toEqual([
      { id: "msg-5", text: "Now the answer", name: "plain" },
    ]);
  });

  it("only the draft the END names is working", () => {
    const out = resolveGroup(
      [drafted("One."), drafted("Two.", { id: "msg-5" })],
      [finalContent(4, ""), workingEnd()],
    );
    expect(out?.drafts.map((d) => d.purpose)).toEqual(["working", undefined]);
  });
});
