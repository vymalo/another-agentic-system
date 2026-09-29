// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  agentMessage,
  errorEvent,
  THREAD_ID,
  userMessage,
} from "@/features/chat/lib/test-fixtures";
import { EVENT_KINDS } from "@/lib/api/types";
import { backoffMs, type EventSourceLike, useEventStream } from "./use-event-stream";

class FakeEventSource implements EventSourceLike {
  static instances: FakeEventSource[] = [];
  readyState = 0;
  onopen: ((ev: Event) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;
  listeners = new Map<string, (ev: MessageEvent<string>) => void>();
  closed = false;
  constructor(readonly url: string) {
    FakeEventSource.instances.push(this);
  }
  addEventListener(type: string, listener: (ev: MessageEvent<string>) => void) {
    this.listeners.set(type, listener);
  }
  close() {
    this.closed = true;
    this.readyState = 2;
  }
  emit(kind: string, payload: unknown) {
    const ev = { type: kind, data: JSON.stringify(payload) } as MessageEvent<string>;
    this.listeners.get(kind)?.(ev);
    // like a real EventSource, a named `error` event also reaches `onerror`
    if (kind === "error") this.onerror?.(ev);
  }
}

const factory = (url: string) => new FakeEventSource(url);

afterEach(() => {
  FakeEventSource.instances = [];
  vi.useRealTimers();
});

const flushMicrotasks = () => act(async () => {});

describe("useEventStream", () => {
  it("opens the thread stream and listens to every event kind", () => {
    renderHook(() => useEventStream(THREAD_ID, { enabled: true, onEvents: () => {}, factory }));
    const es = FakeEventSource.instances[0];
    expect(es?.url).toBe(`/api/threads/${THREAD_ID}/stream`);
    expect([...(es?.listeners.keys() ?? [])].sort()).toEqual([...EVENT_KINDS].sort());
  });

  it("delivers named events in batches and reports the connection", async () => {
    const onEvents = vi.fn();
    const { result } = renderHook(() =>
      useEventStream(THREAD_ID, { enabled: true, onEvents, factory }),
    );
    expect(result.current).toBe("connecting");
    const es = FakeEventSource.instances[0];
    act(() => es?.onopen?.(new Event("open")));
    expect(result.current).toBe("open");
    act(() => {
      es?.emit("user_message", userMessage(1));
      es?.emit("agent_message", agentMessage(2, "m", "hi", true));
    });
    await flushMicrotasks();
    expect(onEvents).toHaveBeenCalledTimes(1);
    expect(onEvents.mock.calls[0]?.[0].map((e: { seq: number }) => e.seq)).toEqual([1, 2]);
  });

  it("treats a server-sent error event as data, not as a connection error", async () => {
    const onEvents = vi.fn();
    const { result } = renderHook(() =>
      useEventStream(THREAD_ID, { enabled: true, onEvents, factory }),
    );
    const es = FakeEventSource.instances[0];
    act(() => es?.onopen?.(new Event("open")));
    act(() => {
      es?.emit("error", errorEvent(2, "Agent crashed"));
      // a genuine connection error is a plain Event without data
      es?.listeners.get("error")?.(new Event("error") as MessageEvent<string>);
    });
    await flushMicrotasks();
    expect(result.current).toBe("open");
    expect(onEvents).toHaveBeenCalledTimes(1);
    expect(onEvents.mock.calls[0]?.[0]).toHaveLength(1);
  });

  it("leaves a retrying (CONNECTING) source alone but marks the connection", () => {
    const { result } = renderHook(() =>
      useEventStream(THREAD_ID, { enabled: true, onEvents: () => {}, factory }),
    );
    const es = FakeEventSource.instances[0];
    act(() => es?.onerror?.(new Event("error")));
    expect(result.current).toBe("reconnecting");
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(es?.closed).toBe(false);
  });

  it("recreates a CLOSED source with backoff", () => {
    vi.useFakeTimers();
    renderHook(() => useEventStream(THREAD_ID, { enabled: true, onEvents: () => {}, factory }));
    const first = FakeEventSource.instances[0];
    act(() => {
      if (first) first.readyState = 2;
      first?.onerror?.(new Event("error"));
    });
    expect(FakeEventSource.instances).toHaveLength(1);
    act(() => vi.advanceTimersByTime(backoffMs(0)));
    expect(FakeEventSource.instances).toHaveLength(2);
    const second = FakeEventSource.instances[1];
    act(() => {
      if (second) second.readyState = 2;
      second?.onerror?.(new Event("error"));
      vi.advanceTimersByTime(backoffMs(0));
    });
    expect(FakeEventSource.instances).toHaveLength(2); // second attempt waits longer
    act(() => vi.advanceTimersByTime(backoffMs(1)));
    expect(FakeEventSource.instances).toHaveLength(3);
  });

  it("closes when disabled and on unmount", () => {
    const { rerender, unmount } = renderHook(
      ({ enabled }) => useEventStream(THREAD_ID, { enabled, onEvents: () => {}, factory }),
      { initialProps: { enabled: true } },
    );
    const es = FakeEventSource.instances[0];
    rerender({ enabled: false });
    expect(es?.closed).toBe(true);
    rerender({ enabled: true });
    const again = FakeEventSource.instances[1];
    expect(again?.closed).toBe(false);
    unmount();
    expect(again?.closed).toBe(true);
  });
});
