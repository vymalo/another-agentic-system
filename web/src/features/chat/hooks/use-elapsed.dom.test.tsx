// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useElapsed, useElapsedAt } from "./use-elapsed";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("useElapsed", () => {
  it("is false while off, false until the time has passed, then true", () => {
    const { result, rerender } = renderHook(({ on }) => useElapsed(on, 1000), {
      initialProps: { on: false },
    });
    expect(result.current).toBe(false);
    rerender({ on: true });
    expect(result.current).toBe(false);
    act(() => vi.advanceTimersByTime(999));
    expect(result.current).toBe(false);
    act(() => vi.advanceTimersByTime(1));
    expect(result.current).toBe(true);
  });

  it("starts over when it was off in between, and is false at once when it goes off", () => {
    const { result, rerender } = renderHook(({ on }) => useElapsed(on, 1000), {
      initialProps: { on: true },
    });
    act(() => vi.advanceTimersByTime(600));
    rerender({ on: false });
    act(() => vi.advanceTimersByTime(600));
    expect(result.current).toBe(false);
    rerender({ on: true });
    act(() => vi.advanceTimersByTime(600));
    expect(result.current).toBe(false); // 600 ms since it came back, not 1200
    act(() => vi.advanceTimersByTime(400));
    expect(result.current).toBe(true);
    rerender({ on: false });
    expect(result.current).toBe(false);
  });
});

describe("useElapsedAt", () => {
  it("is true once it has been on for the time with the same key, and starts over when the key moves", () => {
    const { result, rerender } = renderHook(({ key }) => useElapsedAt(true, key, 1000), {
      initialProps: { key: 3 },
    });
    act(() => vi.advanceTimersByTime(900));
    expect(result.current).toBe(false);
    rerender({ key: 4 }); // something moved: the wait starts again
    act(() => vi.advanceTimersByTime(900));
    expect(result.current).toBe(false);
    act(() => vi.advanceTimersByTime(100));
    expect(result.current).toBe(true);
    rerender({ key: 5 });
    expect(result.current).toBe(false);
  });

  it("is false at once when it goes off", () => {
    const { result, rerender } = renderHook(({ on }) => useElapsedAt(on, 1, 1000), {
      initialProps: { on: true },
    });
    act(() => vi.advanceTimersByTime(1000));
    expect(result.current).toBe(true);
    rerender({ on: false });
    expect(result.current).toBe(false);
  });
});
