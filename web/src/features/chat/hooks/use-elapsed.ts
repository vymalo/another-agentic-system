import { useEffect, useState } from "react";

/**
 * True once `on` has been true for `ms` without a break, and false again as soon as it is not.
 * A finished thread uses it to keep its stream for a while before it lets go.
 */
export function useElapsed(on: boolean, ms: number): boolean {
  const [elapsed, setElapsed] = useState(false);
  useEffect(() => {
    if (!on) {
      setElapsed(false);
      return;
    }
    const timer = setTimeout(() => setElapsed(true), ms);
    return () => clearTimeout(timer);
  }, [on, ms]);
  return on && elapsed;
}

/**
 * `useElapsed` that starts over when `key` changes: true once `on` has been true for `ms` with
 * the same `key`. A stream that goes quiet is one whose `lastSeq` stopped moving.
 */
export function useElapsedAt(on: boolean, key: number, ms: number): boolean {
  const [elapsedFor, setElapsedFor] = useState<number | null>(null);
  useEffect(() => {
    if (!on) {
      setElapsedFor(null);
      return;
    }
    const timer = setTimeout(() => setElapsedFor(key), ms);
    return () => clearTimeout(timer);
  }, [on, key, ms]);
  return on && elapsedFor === key;
}
