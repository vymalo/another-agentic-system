"use client";

import { useAui } from "@assistant-ui/react";
import {
  useAgUiInterrupts,
  useAgUiSendA2uiAction,
  useAgUiSubmitInterruptResponses,
} from "@assistant-ui/react-ag-ui";
import {
  createContext,
  type ReactNode,
  type RefObject,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
} from "react";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import type { ThreadState } from "@/lib/api/types";
import { isTerminal } from "@/lib/api/types";

/** What a Button of a surface sends to the agent (`forwardedProps.a2uiAction.userAction`). */
export type UserAction = {
  name: string;
  surfaceId: string;
  sourceComponentId: string;
  context: Record<string, unknown>;
};

/**
 * What the surfaces need from the app. `canSend`: the thread waits for the owner (`blocked`), so
 * the orchestrator accepts an action, and none is on its way already. `canCompose`: the message
 * box takes text (the thread is not finished). `send` and `fillComposer` are called by a click on
 * a rendered control, and by nothing else.
 */
export type SurfaceHost = {
  state: ThreadState | undefined;
  canSend: boolean;
  canCompose: boolean;
  send: (action: UserAction) => void;
  /** Put agent-authored text in the message box, focused and unsent. */
  fillComposer: (text: string) => void;
  /** An action the app will not send (over the size limit): say so in the composer's error line. */
  reject: (message: string) => void;
};

const INERT: SurfaceHost = {
  state: undefined,
  canSend: false,
  canCompose: false,
  send: () => {},
  fillComposer: () => {},
  reject: () => {},
};

const Ctx = createContext<SurfaceHost>(INERT);
export const useSurfaceHost = (): SurfaceHost => useContext(Ctx);
export const SurfaceHostContext = Ctx;

type Props = {
  agent: ThreadAgent;
  /** What the server says the thread is doing. */
  state: ThreadState | undefined;
  /** The message box's textarea, to focus it. */
  composerRef: RefObject<HTMLTextAreaElement | null>;
  /** A send the app refuses before the server sees it. */
  onRejected: (message: string) => void;
  children: ReactNode;
};

/**
 * Mounted inside the runtime provider. An action goes through the runtime, so the run it starts
 * is in the transcript like any other (ADR 0013):
 *  - no interrupt open: the runtime's own `sendA2uiAction` (`useAgUiSendA2uiAction`);
 *  - an interrupt open (the usual case: the agent asked and the surface is the answer): the
 *    runtime refuses `sendA2uiAction`, so the interrupt is closed through the runtime and the
 *    action is staged on the `ThreadAgent`, which sends it instead of the `resume`.
 * Either way the request has no message and no `resume`, and only one action is in flight.
 */
export function SurfaceHostProvider({ agent, state, composerRef, onRejected, children }: Props) {
  const aui = useAui();
  const interrupts = useAgUiInterrupts();
  const sendA2uiAction = useAgUiSendA2uiAction();
  const submitInterrupts = useAgUiSubmitInterruptResponses();
  const { sendFailures } = useSyncExternalStore(
    agent.onChange,
    agent.getSnapshot,
    agent.getSnapshot,
  );
  const [busy, setBusy] = useState(false);

  // An action is on its way until the thread moves on, or the server refused it.
  // biome-ignore lint/correctness/useExhaustiveDependencies: both are the triggers, not inputs
  useEffect(() => setBusy(false), [state, sendFailures]);

  const canSend = state === "blocked" && !busy;
  const canCompose = state !== undefined && !isTerminal(state);

  const send = useCallback(
    (action: UserAction) => {
      if (!canSend) return;
      setBusy(true);
      if (interrupts.length === 0) {
        try {
          sendA2uiAction({ ...action });
        } catch (e) {
          setBusy(false);
          onRejected(e instanceof Error ? e.message : String(e));
        }
        return;
      }
      agent.stageA2uiAction({ ...action });
      submitInterrupts(interrupts.map((i) => ({ interruptId: i.id, status: "cancelled" as const })))
        .catch((e: unknown) => {
          setBusy(false);
          onRejected(e instanceof Error ? e.message : String(e));
        })
        .finally(() => agent.clearStagedAction());
    },
    [canSend, interrupts, sendA2uiAction, submitInterrupts, agent, onRejected],
  );

  const fillComposer = useCallback(
    (text: string) => {
      aui.composer().setText(text);
      composerRef.current?.focus();
    },
    [aui, composerRef],
  );

  const value = useMemo<SurfaceHost>(
    () => ({ state, canSend, canCompose, send, fillComposer, reject: onRejected }),
    [state, canSend, canCompose, send, fillComposer, onRejected],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
