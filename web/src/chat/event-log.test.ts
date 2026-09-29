import { describe, expect, it, vi } from "vitest";
import type { ApiEvent } from "@/api/types";
import { applyEvents, emptyLog, lastThreadStateEvent, visibleEvents } from "./event-log";
import { agentMessage, artifact, status, THREAD_ID, threadState, userMessage } from "./fixtures";

const seqs = (events: readonly { seq: number }[]) => events.map((e) => e.seq);

describe("applyEvents", () => {
  it("renders out-of-order and duplicate events once, in seq order", () => {
    const e = (n: number) => userMessage(n, `m${n}`);
    let log = emptyLog(THREAD_ID);
    log = applyEvents(log, [e(3), e(1)]);
    log = applyEvents(log, [e(2), e(2), e(1)]);
    log = applyEvents(log, [e(3)]);
    expect(seqs(log.ordered)).toEqual([1, 2, 3]);
    expect(seqs(visibleEvents(log))).toEqual([1, 2, 3]);
    expect(log.lastSeq).toBe(3);
  });

  it("dedupes within a single batch", () => {
    const log = applyEvents(emptyLog(THREAD_ID), [userMessage(1), userMessage(1), userMessage(2)]);
    expect(seqs(log.ordered)).toEqual([1, 2]);
  });

  it("does not duplicate anything on a reconnect replay", () => {
    const all = [
      userMessage(1),
      status(2, "working"),
      agentMessage(3, "m1", "Hi", true),
      artifact(4, "https://github.com/o/r/pull/1"),
      status(5, "completed"),
      threadState(6, "done"),
    ];
    let log = applyEvents(emptyLog(THREAD_ID), all.slice(0, 5));
    // a fresh EventSource has no Last-Event-ID: full replay from seq 1
    log = applyEvents(log, all);
    expect(seqs(log.ordered)).toEqual([1, 2, 3, 4, 5, 6]);
    // a resumed EventSource replays only what follows Last-Event-ID
    const again = applyEvents(log, all.slice(3));
    expect(again).toBe(log);
  });

  it("returns the same object when nothing changed", () => {
    const log = applyEvents(emptyLog(THREAD_ID), [userMessage(1)]);
    expect(applyEvents(log, [userMessage(1)])).toBe(log);
    expect(applyEvents(log, [])).toBe(log);
  });

  it("ignores other threads and malformed events", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const other: ApiEvent = { ...userMessage(1), threadId: "22222222-2222-4222-8222-222222222222" };
    const malformed: ApiEvent = { ...userMessage(2), data: { nope: true } };
    const badState: ApiEvent = { ...threadState(3, "running") };
    const log = applyEvents(emptyLog(THREAD_ID), [other, malformed, badState, userMessage(4)]);
    expect(seqs(log.ordered)).toEqual([4]);
    warn.mockRestore();
  });
});

describe("visibleEvents", () => {
  it("replaces a partial agent_message with its final version, in place", () => {
    const log = applyEvents(emptyLog(THREAD_ID), [
      userMessage(1),
      agentMessage(2, "m1", "Hel", false),
      status(3, "working"),
      agentMessage(4, "m1", "Hello", true),
    ]);
    const visible = visibleEvents(log);
    expect(seqs(visible)).toEqual([1, 4, 3]);
    const msg = visible[1];
    expect(msg?.kind === "agent_message" && msg.data).toMatchObject({ text: "Hello", final: true });
  });

  it("keeps distinct messageIds separate and drops thread_state", () => {
    const log = applyEvents(emptyLog(THREAD_ID), [
      agentMessage(1, "a", "one", true),
      threadState(2, "blocked"),
      agentMessage(3, "b", "two", true),
    ]);
    expect(seqs(visibleEvents(log))).toEqual([1, 3]);
  });

  it("does not reorder when a partial arrives after its final", () => {
    const log = applyEvents(emptyLog(THREAD_ID), [
      agentMessage(4, "m1", "Hello", true),
      agentMessage(2, "m1", "Hel", false),
    ]);
    const visible = visibleEvents(log);
    expect(visible).toHaveLength(1);
    expect(visible[0]?.seq).toBe(4);
  });
});

describe("lastThreadStateEvent", () => {
  it("returns the newest thread_state", () => {
    const log = applyEvents(emptyLog(THREAD_ID), [
      userMessage(1),
      threadState(2, "blocked"),
      threadState(5, "done"),
    ]);
    expect(lastThreadStateEvent(log)).toEqual({ state: "done", seq: 5 });
    expect(lastThreadStateEvent(emptyLog(THREAD_ID))).toBeUndefined();
  });
});
