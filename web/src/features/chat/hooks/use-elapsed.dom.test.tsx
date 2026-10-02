// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useElapsed } from "./use-elapsed";

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
