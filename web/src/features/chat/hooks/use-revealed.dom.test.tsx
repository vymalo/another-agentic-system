// @vitest-environment jsdom
import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useRevealed } from "./use-revealed";

describe("useRevealed", () => {
  it("is false until the thread is settled", () => {
    const { result } = renderHook(({ settled }) => useRevealed(settled), {
      initialProps: { settled: false },
    });
    expect(result.current).toBe(false);
  });

  it("is true from the render in which the thread settles", () => {
    const { result, rerender } = renderHook(({ settled }) => useRevealed(settled), {
      initialProps: { settled: false },
    });
    rerender({ settled: true });
    expect(result.current).toBe(true);
  });

  it("stays true when a run that starts later makes the thread unsettled again", () => {
    const { result, rerender } = renderHook(({ settled }) => useRevealed(settled), {
      initialProps: { settled: true },
    });
    expect(result.current).toBe(true);
    rerender({ settled: false });
    expect(result.current).toBe(true);
  });
});
