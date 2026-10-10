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
import { parseArtifact } from "@/features/chat/lib/agui/vymalo";
import { type KeptFile, keptFileOf } from "@/features/chat/lib/files";

type Runtime = Pick<AgUiAssistantRuntime, "thread">;

/**
 * What the turns that are not held contribute to the readouts that cover the whole thread (docs/api/history.md, "Carry"): their
 * number, which numbers the turns that are, and their kept files, which an `Image` of a surface may name.
 */
export function useCarried(agent: ThreadAgent): {
  turnsBefore: number;
  carriedFiles: readonly KeptFile[];
} {
  const history = useSyncExternalStore(agent.onHistoryChange, agent.getHistory, agent.getHistory);
  const carriedFiles = useMemo(
    () =>
      history.files.flatMap((content) => {
        const artifact = parseArtifact(content);
        const file = artifact ? keptFileOf(artifact) : undefined;
        return file ? [file] : [];
      }),
    [history.files],
  );
  return { turnsBefore: history.turnsBefore, carriedFiles };
}

/**
 * The most pages one "load all" reads before it stops and leaves the rest to the next click: the pages double up to the
 * server's largest (20, 40, 80, 100, 100 turns with the defaults), so this is a few hundred turns, not the whole of a thread
 * of ten thousand, which would hold all of it in memory and in a transcript the library draws again at every import.
 */
export const LOAD_ALL_PAGES = 5;

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

  /** One older page: read, made into messages, and put in front of the transcript when it may be replaced. */
  const page = useCallback(async () => {
    // read and checked, not held: the account of the pages, the usage and the carry take it in once the transcript has it, so a
    // page that waits for a moment to import, or whose import fails, leaves what the screen says as it was and is asked again
    const read = await agent.readEarlier();
    // the page is followed by the turns held: a question it ended on has been answered by what comes next
    const older = await buildMessages(read.runs, agent.threadId, true);
    const idle = () => agent.idleForImport() && !latest.current.thread.getState().isRunning;
    // The moment is looked at again once the runs are held: what was idle when the wait ended may not be by the time this
    // continues (a run that begins in between is on its way to the runtime, and the import would lose its message).
    let release: () => void;
    for (;;) {
      if (!idle()) {
        setState("waiting");
        await until(agent, latest.current, idle);
        setState("loading");
      }
      release = agent.pauseRuns();
      if (idle()) break;
      release();
    }
    try {
      const thread = latest.current.thread;
      const current = thread.export().messages.map((m) => m.message);
      const joined = joinMessages(older, current);
      anchor.current?.capture();
      thread.import(asRepository(joined));
      read.commit();
      await untilHeld(thread, joined.length);
    } finally {
      release();
    }
  }, [agent]);

  /** One page, or up to `LOAD_ALL_PAGES` of them (`every`); one run at a time. */
  const run = useCallback(
    (every: boolean) => {
      if (busy.current || !agent.getHistory().earlier) return;
      busy.current = true;
      setError(null);
      void (async () => {
        try {
          let pages = 0;
          do {
            setState("loading");
            await page();
            pages++;
          } while (every && agent.getHistory().earlier && pages < LOAD_ALL_PAGES);
          setState("idle");
        } catch (e) {
          setError(e instanceof Error ? e.message : String(e));
          setState("error");
        } finally {
          busy.current = false;
        }
      })();
    },
    [agent, page],
  );
  const load = useCallback(() => run(false), [run]);
  const loadAll = useCallback(() => run(true), [run]);

  return useMemo(
    () => ({
      earlier: history.earlier,
      state,
      error,
      load,
      loadAll,
      anchorMissed: history.anchorMissed,
      anchor,
    }),
    [history.earlier, history.anchorMissed, state, error, load, loadAll],
  );
}
