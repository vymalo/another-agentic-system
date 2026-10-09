import type { AgUiAssistantRuntime } from "@assistant-ui/react-ag-ui";
import {
  type MutableRefObject,
  useCallback,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { Anchor, EarlierControl, EarlierState } from "@/features/chat/components/earlier";
import { untilHeld } from "@/features/chat/lib/agui/live-runs";
import { asRepository, buildMessages, joinMessages } from "@/features/chat/lib/agui/seed";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";

type Runtime = Pick<AgUiAssistantRuntime, "thread">;

/** Waits until `ready()` holds, looking again whenever the agent or the runtime says something changed. */
function until(agent: ThreadAgent, runtime: Runtime, ready: () => boolean): Promise<void> {
  return new Promise((resolve) => {
    const stops: (() => void)[] = [];
    const look = () => {
      if (!ready()) return;
      for (const stop of stops) stop();
      resolve();
    };
    stops.push(agent.onChange(look), runtime.thread.subscribe(look));
    look();
  });
}

/**
 * The older turns of a thread opened at its end (ADR 0059): reads the next older page, makes its messages, waits for a
 * moment the transcript may be replaced, and puts them in front of it in one import, keeping the place of the turn the
 * person is looking at (`anchor`).
 *
 * The moment: no run is open or on its way, nothing is being sent and no action on a surface is staged (`idleForImport`),
 * and the runtime is not running. An import in the middle of any of those loses the message of the open run, a staged click
 * and the reply it waits for (measured, ADR 0059); the row says the rest will load when the agent is done, and it does.
 * A question or form that waits for the person does not hold it back: its message is in the transcript that is imported.
 */
export function useEarlier(agent: ThreadAgent, runtime: Runtime): EarlierControl {
  const history = useSyncExternalStore(agent.onHistoryChange, agent.getHistory, agent.getHistory);
  const [state, setState] = useState<EarlierState>("idle");
  const [error, setError] = useState<string | null>(null);
  const busy = useRef(false);
  const anchor: MutableRefObject<Anchor | null> = useRef(null);
  const latest = useRef(runtime);
  latest.current = runtime;

  const load = useCallback(() => {
    if (busy.current || !agent.getHistory().earlier) return;
    busy.current = true;
    setError(null);
    setState("loading");
    void (async () => {
      try {
        const runs = await agent.fetchEarlier();
        const older = await buildMessages(runs, agent.threadId);
        const idle = () => agent.idleForImport() && !latest.current.thread.getState().isRunning;
        if (!idle()) {
          setState("waiting");
          await until(agent, latest.current, idle);
          setState("loading");
        }
        const release = agent.pauseRuns();
        try {
          const thread = latest.current.thread;
          const current = thread.export().messages.map((m) => m.message);
          const joined = joinMessages(older, current);
          anchor.current?.capture();
          thread.import(asRepository(joined));
          await untilHeld(thread, joined.length);
        } finally {
          release();
        }
        setState("idle");
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setState("error");
      } finally {
        busy.current = false;
      }
    })();
  }, [agent]);

  return useMemo(
    () => ({ earlier: history.earlier, state, error, load, anchor }),
    [history.earlier, state, error, load],
  );
}
