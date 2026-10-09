import { useRef } from "react";

/**
 * Whether the transcript of an opened thread has been shown (ADR 0059, slice 1), sticky: once it is, a run that
 * starts later is the live conversation, not a replay to hold back. `settled` is `isSettled` (lib/reveal.ts).
 */
export function useRevealed(settled: boolean): boolean {
  const revealed = useRef(false);
  if (settled) revealed.current = true;
  return revealed.current;
}
