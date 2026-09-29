"use client";

import { useAgUiSteerAway } from "@assistant-ui/react-ag-ui";
import { useEffect, useRef } from "react";
import { driveExternalRuns, type LiveRunsRuntime } from "@/features/chat/lib/agui/live-runs";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";

/**
 * Renders nothing: hands the runs nobody here started (the thread's replay at load, another tab's
 * runs, a producer's own) to the runtime. Mounted inside `AssistantRuntimeProvider`; the loop is
 * lib/agui/live-runs.ts.
 */
export function LiveRuns({ agent, runtime }: { agent: ThreadAgent; runtime: LiveRunsRuntime }) {
  const steerAway = useAgUiSteerAway();
  const latest = useRef({ runtime, steerAway });
  latest.current = { runtime, steerAway };

  useEffect(() => {
    const stop = new AbortController();
    void driveExternalRuns(
      agent,
      () => latest.current.runtime,
      () => latest.current.steerAway,
      stop.signal,
    );
    return () => stop.abort();
  }, [agent]);
  return null;
}
