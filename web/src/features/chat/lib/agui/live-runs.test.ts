import { afterEach, describe, expect, it, vi } from "vitest";
import { quiesce } from "./live-runs";

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

describe("quiesce", () => {
  it("resolves true once the thread is idle and unchanged, and stops listening", async () => {
    const t = fakeThread();
    await expect(quiesce(t.thread, 1_000, 5)).resolves.toBe(true);
    expect(t.subscribers.size).toBe(0);
  });

  it("waits for a run to end and for the message count to stop moving", async () => {
    const t = fakeThread();
    t.state.isRunning = true;
    let done = false;
    const waiting = quiesce(t.thread, 1_000, 10).then((v) => {
      done = true;
      return v;
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(done).toBe(false);
    t.change({ isRunning: false, messages: [1] });
    await new Promise((r) => setTimeout(r, 4));
    t.change({ messages: [1, 2] }); // the transcript was still catching up
    await expect(waiting).resolves.toBe(true);
    expect(t.state.messages).toHaveLength(2);
  });

  it("does not settle before the message the caller appended shows", async () => {
    const t = fakeThread();
    t.state.messages = [1, 2];
    let done = false;
    const waiting = quiesce(t.thread, 1_000, 5, 3).then((v) => {
      done = true;
      return v;
    });
    await new Promise((r) => setTimeout(r, 30)); // quiet, but the append has not landed
    expect(done).toBe(false);
    t.change({ messages: [1, 2, 3] });
    await expect(waiting).resolves.toBe(true);
  });

  it("warns and gives up, without spinning, when the thread never stops running", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const t = fakeThread();
    t.state.isRunning = true;
    const getState = vi.spyOn(t.thread, "getState");
    await expect(quiesce(t.thread, 40, 5)).resolves.toBe(false);
    expect(warn).toHaveBeenCalledTimes(1);
    expect(String(warn.mock.calls[0]?.[0])).toContain("did not settle");
    // it looked once at the start, and not once per tick
    expect(getState.mock.calls.length).toBeLessThan(5);
    expect(t.subscribers.size).toBe(0);
  });
});
