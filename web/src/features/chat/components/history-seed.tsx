"use client";

import type { AgUiAssistantRuntime } from "@assistant-ui/react-ag-ui";
import { useEffect, useRef } from "react";
import { untilHeld } from "@/features/chat/lib/agui/live-runs";
import { asRepository, buildMessages } from "@/features/chat/lib/agui/seed";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";

/**
 * Renders nothing: puts the newest turns of a thread opened at its end (ADR 0059) in the runtime. The agent read them as a
 * page of history and holds their runs (`takeSeed`); their messages are made without the runtime (`buildMessages`) and
 * imported in one step, and then the agent follows the stream from where the page ended (`seeded`). Mounted inside
 * `AssistantRuntimeProvider`, beside `LiveRuns`.
 */
export function HistorySeed({
  agent,
  runtime,
}: {
  agent: ThreadAgent;
  runtime: Pick<AgUiAssistantRuntime, "thread">;
}) {
  const latest = useRef(runtime);
  latest.current = runtime;
  useEffect(() => {
    const seed = async () => {
      const runs = agent.takeSeed();
      if (!runs) return;
      try {
        const messages = await buildMessages(runs, agent.threadId);
        const thread = latest.current.thread;
        if (messages.length > 0) {
          thread.import(asRepository(messages));
          await untilHeld(thread, messages.length);
        }
        agent.seeded();
      } catch (e) {
        agent.seedFailed(e);
      }
    };
    void seed();
    return agent.onHistoryChange(() => void seed());
  }, [agent]);
  return null;
}
