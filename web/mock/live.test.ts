import { describe, expect, it } from "vitest";
import type { components } from "../src/lib/api/schema";
import { LiveOverlay, type LivePiece } from "./live";
import { type Frame, Projector } from "./projection";

/**
 * The mock's live overlay on the rules of the real one (`orch-agui-projection`, `tests/live.rs`):
 * the golden `stream` is the one end-to-end proof (golden.test.ts); these are the cases it does not
 * reach: a stream given up, one that is held for an invocation, one cut by the end of the run.
 */
type Event = components["schemas"]["Event"];

const AGENT = { type: "agent", name: "plain" } as const;
const SYSTEM = { type: "system", name: "orchestrator" } as const;
const at = "2027-01-15T08:00:00Z";

const ev = (
  seq: number,
  kind: Event["kind"],
  actor: Event["actor"],
  data: Event["data"],
): Event => ({
  seq,
  threadId: "t",
  at,
  kind,
  actor,
  data,
});
const user = ev(1, "user_message", { type: "user", name: "u" }, { text: "go" });
const working = ev(2, "agent_status", AGENT, { status: "working" });
const message = (seq: number, id: string, text: string) =>
  ev(seq, "agent_message", AGENT, { messageId: id, final: true, text });
const completed = (seq: number, detail?: string) =>
  ev(seq, "agent_status", AGENT, { status: "completed", ...(detail ? { detail } : {}) });
const done = (seq: number) => ev(seq, "thread_state", SYSTEM, { state: "done" });

const piece = (
  id: string,
  offset: number,
  text: string,
  end: LivePiece["end"] = "open",
): LivePiece => ({
  messageId: id,
  agent: "plain",
  offset,
  text,
  end,
});

/** A projector, an overlay and the events applied to both, as a viewer's connection does. */
function connection() {
  const projector = new Projector({ threadId: "t", title: "go", target: { agentId: "plain" } });
  const overlay = new LiveOverlay();
  return {
    projector,
    overlay,
    log: (event: Event): Frame[] => overlay.logged(projector, projector.apply(event)),
    live: (p: LivePiece): Frame[] => overlay.live(projector, p),
  };
}

const show = (frames: Frame[]) =>
  frames.map((f) => {
    const e = f.event;
    const live = (e.metadata as { "vymalo.live"?: unknown } | undefined)?.["vymalo.live"];
    return [
      e.type,
      e.messageId ?? e.subagentRunId ?? "",
      e.delta ?? "",
      JSON.stringify(live ?? null),
      f.id ?? "",
    ].join("|");
  });

describe("the mock's live overlay", () => {
  it("opens at offset 0, grows, and the log's message continues it with the resume point on its END", () => {
    const c = connection();
    c.log(user);
    c.log(working);
    expect(show(c.live(piece("m", 0, "Fib")))).toEqual([
      "TEXT_MESSAGE_START|m||{}|",
      'TEXT_MESSAGE_CONTENT|m|Fib|{"offset":0}|',
    ]);
    expect(show(c.live(piece("m", 3, "onacci")))).toEqual([
      'TEXT_MESSAGE_CONTENT|m|onacci|{"offset":3}|',
    ]);
    expect(show(c.log(message(3, "m", "Fibonacci!")))).toEqual([
      'TEXT_MESSAGE_CONTENT|m|!|{"offset":9,"final":true}|',
      'TEXT_MESSAGE_END|m||{"final":true}|3',
    ]);
  });

  it("a final that does not start with what was said replaces it", () => {
    const c = connection();
    c.log(user);
    c.log(working);
    c.live(piece("m", 0, "Fibbonacci"));
    expect(show(c.log(message(3, "m", "Fibonacci")))[0]).toBe(
      'TEXT_MESSAGE_CONTENT|m|Fibonacci|{"offset":0,"final":true}|',
    );
  });

  it("an overlap is trimmed, a repeat and a gap say nothing, and only offset 0 opens", () => {
    const c = connection();
    c.log(user);
    c.log(working);
    expect(c.live(piece("m", 4, "late"))).toEqual([]);
    c.live(piece("m", 0, "Fibonacci"));
    expect(show(c.live(piece("m", 3, "onacci in")))).toEqual([
      'TEXT_MESSAGE_CONTENT|m| in|{"offset":9}|',
    ]);
    expect(c.live(piece("m", 0, "Fib"))).toEqual([]);
    expect(c.live(piece("m", 20, "gap"))).toEqual([]);
  });

  it("a stream given up ends with the flag, its late pieces are ignored, and the log's message is said under another id", () => {
    const c = connection();
    c.log(user);
    c.log(working);
    c.live(piece("m", 0, "forty-"));
    expect(show(c.live(piece("m", 6, "", "abandoned")))).toEqual([
      'TEXT_MESSAGE_END|m||{"abandoned":true}|',
    ]);
    expect(c.live(piece("m", 6, "two"))).toEqual([]);
    const said = show(c.log(message(3, "m", "forty-two")));
    expect(said.map((s) => s.split("|").slice(0, 3).join("|"))).toEqual([
      "TEXT_MESSAGE_START|m~final|",
      "TEXT_MESSAGE_CONTENT|m~final|forty-two",
      "TEXT_MESSAGE_END|m~final|",
    ]);
    expect(said[0]).not.toContain("vymalo.live");
  });

  it("an open stream is given up before the invocation closes", () => {
    const c = connection();
    c.log(user);
    c.log(working);
    c.live(piece("m", 0, "half"));
    const frames = c.log(completed(3));
    const kinds = frames.map((f) => f.event.type);
    expect(kinds.indexOf("TEXT_MESSAGE_END")).toBeGreaterThanOrEqual(0);
    expect(kinds.indexOf("TEXT_MESSAGE_END")).toBeLessThan(kinds.indexOf("SUBAGENT_FINISHED"));
    expect(show(frames)[kinds.indexOf("TEXT_MESSAGE_END")]).toContain('{"abandoned":true}');
  });

  it("another stream opening ends the open one as given up", () => {
    const c = connection();
    c.log(user);
    c.log(working);
    c.live(piece("a", 0, "one"));
    expect(
      show(c.live(piece("b", 0, "two"))).map((s) => s.split("|").slice(0, 2).join("|")),
    ).toEqual(["TEXT_MESSAGE_END|a", "TEXT_MESSAGE_START|b", "TEXT_MESSAGE_CONTENT|b"]);
  });

  it("a piece for a message the log has said is late; one before the invocation is held and said after the event that opens it", () => {
    const c = connection();
    c.log(user);
    // no invocation yet (the agent has said nothing): held
    expect(c.live(piece("m", 0, "Fib"))).toEqual([]);
    const frames = c.log(working);
    expect(frames.map((f) => f.event.type).slice(-2)).toEqual([
      "TEXT_MESSAGE_START",
      "TEXT_MESSAGE_CONTENT",
    ]);
    // the log says it, so the next piece of it is late
    c.log(message(3, "m", "Fibonacci"));
    expect(c.live(piece("m", 3, "onacci"))).toEqual([]);
    c.log(completed(4));
    c.log(done(5));
  });

  it("held pieces are dropped when the run closes, and nothing is kept with no run open", () => {
    const c = connection();
    expect(c.live(piece("m", 0, "Fib"))).toEqual([]);
    c.log(user);
    c.log(completed(2));
    c.log(done(3));
    expect(c.live(piece("n", 0, "late"))).toEqual([]);
  });

  it("the words of another agent are not this invocation's", () => {
    const c = connection();
    c.log(user);
    c.log(working);
    expect(c.live({ ...piece("m", 0, "hi"), agent: "someone-else" })).toEqual([]);
  });
});
