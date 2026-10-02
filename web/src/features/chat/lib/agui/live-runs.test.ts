import { afterEach, describe, expect, it, vi } from "vitest";
import { untilShown } from "./live-runs";

afterEach(() => vi.restoreAllMocks());

/** A thread that holds a transcript and tells its subscribers when it changes. */
function fakeThread() {
  const state = { isRunning: false, messages: [] as number[] };
  const subscribers = new Set<() => void>();
  return {
    state,
    subscribers,
    thread: {
      getState: () => state as never,
      subscribe: (cb: () => void) => {
        subscribers.add(cb);
        return () => subscribers.delete(cb);
      },
    },
    change: (next: Partial<typeof state>) => {
      Object.assign(state, next);
      for (const cb of [...subscribers]) cb();
    },
  };
}

describe("untilShown", () => {
  it("resolves true at once when the thread is idle and shows enough, and stops listening", async () => {
    const t = fakeThread();
    t.state.messages = [1, 2];
    await expect(untilShown(t.thread, 2, 1_000)).resolves.toBe(true);
    expect(t.subscribers.size).toBe(0);
  });

  it("waits for the messages to show, however long that takes", async () => {
    const t = fakeThread();
    t.state.messages = [1, 2];
    let done = false;
    const waiting = untilShown(t.thread, 3, 1_000).then((v) => {
      done = true;
      return v;
    });
    await new Promise((r) => setTimeout(r, 60)); // quiet for a long time, but the render has not come
    expect(done).toBe(false);
    t.change({ messages: [1, 2, 3] });
    await expect(waiting).resolves.toBe(true);
    expect(t.subscribers.size).toBe(0);
  });

  it("goes on the moment the messages show: no settle time", async () => {
    const t = fakeThread();
    const waiting = untilShown(t.thread, 1, 1_000);
    t.change({ messages: [1] });
    await expect(waiting).resolves.toBe(true);
  });

  it("waits for a run to end, even when the messages are there", async () => {
    const t = fakeThread();
    t.state.isRunning = true;
    t.state.messages = [1, 2];
    let done = false;
    const waiting = untilShown(t.thread, 2, 1_000).then((v) => {
      done = true;
      return v;
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(done).toBe(false);
    t.change({ isRunning: false });
    await expect(waiting).resolves.toBe(true);
  });

  it("warns and gives up, without spinning, when the messages never show", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const t = fakeThread();
    t.state.messages = [1];
    const getState = vi.spyOn(t.thread, "getState");
    await expect(untilShown(t.thread, 2, 40)).resolves.toBe(false);
    expect(warn).toHaveBeenCalledTimes(1);
    expect(String(warn.mock.calls[0]?.[0])).toContain("did not show 2 messages");
    // it looked once at the start, and not once per tick
    expect(getState.mock.calls.length).toBeLessThan(5);
    expect(t.subscribers.size).toBe(0);
  });
});
