"use client";

import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import { useThreadView } from "@/features/chat/components/thread-view";
import type { Tier } from "@/features/tier/lib/tier";
import type { ApiThread } from "@/lib/api/types";
import { type ChildSessions, useChildSessions } from "../hooks/use-child-sessions";
import type { StartDraft } from "../hooks/use-start-session";
import { childOf, type SessionCard } from "../lib/cards";
import { type AppliedClose, MAX_WAITING_STARTS } from "../lib/children";
import { CHILD_SESSIONS_URI, type ChildSessionsOffer, parseChildSessionsOffer } from "../lib/offer";
import { CloseSessionDialog } from "./close-session-dialog";
import { NewSubagentDialog } from "./new-subagent-dialog";

/** What the panel's Subagents tab and the start line read of the thread's subagents, and the two dialogs they open. */
export type SessionsController = {
  /** The tab is shown: the agent offers subagents, or the thread has some; never on a subagent's own thread. */
  visible: boolean;
  children: ChildSessions;
  /** What the card offers now; null when it does not (or did not, after a refusal). */
  offer: ChildSessionsOffer | null;
  /** The person may start and close: a thread that is theirs and that they may write to. */
  canWrite: boolean;
  /** "New subagent" is shown and can be used; else why not (a sentence), or null when it is not shown at all. */
  newBlocked: { reason: string } | null;
  canStart: boolean;
  openNew: (opener: HTMLElement | null) => void;
  /** A start that failed still has its draft: "Try again" reopens the dialog with it. */
  canRetry: (request: string) => boolean;
  retry: (request: string, opener: HTMLElement | null) => void;
  askClose: (card: SessionCard, opener: HTMLElement | null) => void;
  /**
   * Close is offered for this subagent: it is not closed, and it is not already closing, except that
   * a close that waits for the round to end (`cancel: false`) can be made stronger, "Stop and close",
   * until one that stops the round has been asked.
   */
  closeOffered: (card: SessionCard) => boolean;
  /** What a close this page asked said, by child thread: a subagent's own thread reads its own. */
  applied: ReadonlyMap<string, AppliedClose>;
};

const Context = createContext<SessionsController | null>(null);

/** The open thread's subagents; null outside a thread (and in a test that mounts none). */
export const useSessions = (): SessionsController | null => useContext(Context);

type Props = {
  /** `null` is the new-chat page: no subagents. */
  threadId: string | null;
  /** The open thread, as the resource says it: a subagent's own thread has no subagents. */
  thread: ApiThread | null;
  /** The person's list (the rail), whose rows with `childOf` are the truth about the subagents. */
  rows: readonly ApiThread[];
  /** The thread's agent card, read live. */
  capabilities: AgentCapabilities;
  /** The tier chip's current pick: a start is sent with it. */
  tier: Tier | undefined;
  /** The roles or the owner say the thread is read-only. */
  readOnly: boolean;
  /** Read the list again (a close was accepted). */
  refreshRows: () => void;
  children: ReactNode;
};

export function SessionsProvider({ threadId, children, ...rest }: Props) {
  if (threadId === null) return <>{children}</>;
  return (
    <Active threadId={threadId} {...rest}>
      {children}
    </Active>
  );
}

function Active({
  threadId,
  thread,
  rows,
  capabilities,
  tier,
  readOnly,
  refreshRows,
  children,
}: Props & { threadId: string }) {
  const { sessionStarts } = useThreadView();
  const entry =
    capabilities.status === "ready" ? capabilities.entry?.(CHILD_SESSIONS_URI) : undefined;
  const offered = useMemo(
    () => (entry === undefined ? null : parseChildSessionsOffer(entry)),
    [entry],
  );
  // a refusal that says the card offers nothing hides the button until the card, read again, says otherwise
  const [withdrawn, setWithdrawn] = useState(false);
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new reading of the card ends it
  useEffect(() => setWithdrawn(false), [entry]);
  const offer = withdrawn ? null : offered;
  const isChild = childOf(thread) !== null;
  const canWrite = !readOnly && !isChild;
  // a subagent's own thread closes it from its banner: the person may write there, though they may not start
  const mayClose = !readOnly;

  // the requests this page sent that the stream has not said yet, and what each was (kept until its outcome)
  const [local, setLocal] = useState<readonly string[]>([]);
  const [drafts, setDrafts] = useState<ReadonlyMap<string, StartDraft>>(new Map());
  const kids = useChildSessions(threadId, rows, local);
  useEffect(() => {
    const said = sessionStarts ?? new Map();
    setLocal((l) => (l.some((r) => said.has(r)) ? l.filter((r) => !said.has(r)) : l));
    setDrafts((d) => {
      const kept = [...d].filter(([r]) => said.get(r)?.state !== "opened");
      return kept.length === d.size ? d : new Map(kept);
    });
  }, [sessionStarts]);

  const opener = useRef<HTMLElement | null>(null);
  const [newOpen, setNewOpen] = useState(false);
  const [retrying, setRetrying] = useState<{ request: string; draft: StartDraft } | null>(null);
  const [closing, setClosing] = useState<SessionCard | null>(null);
  const [closeOpen, setCloseOpen] = useState(false);

  const openNew = useCallback((from: HTMLElement | null) => {
    opener.current = from;
    setRetrying(null);
    setNewOpen(true);
  }, []);
  const canRetry = useCallback(
    (request: string) => canWrite && offer !== null && drafts.has(request),
    [canWrite, offer, drafts],
  );
  const retry = useCallback(
    (request: string, from: HTMLElement | null) => {
      const draft = drafts.get(request);
      if (!draft) return;
      opener.current = from;
      setRetrying({ request, draft });
      setNewOpen(true);
    },
    [drafts],
  );
  const askClose = useCallback((card: SessionCard, from: HTMLElement | null) => {
    opener.current = from;
    setClosing(card);
    setCloseOpen(true);
  }, []);

  const forced = kids.forced;
  const closeOffered = useCallback(
    (card: SessionCard) =>
      mayClose &&
      card.threadId !== undefined &&
      card.state !== "closed" &&
      (!(card.closing || card.closeAsked) ||
        (card.state === "working" && !forced.has(card.threadId))),
    [mayClose, forced],
  );

  const noRoom =
    offer !== null && kids.openCount >= offer.maxOpen
      ? `${offer.maxOpen} ${offer.maxOpen === 1 ? "subagent is" : "subagents are"} open. Close one to start another.`
      : kids.waiting >= MAX_WAITING_STARTS
        ? `${MAX_WAITING_STARTS} subagents are starting. Wait for one to open.`
        : null;
  const canStart = canWrite && offer !== null;
  const value = useMemo<SessionsController>(
    () => ({
      visible:
        !isChild &&
        (offer !== null || kids.open.length + kids.closed.length + kids.pending.length > 0),
      children: kids,
      offer,
      canWrite,
      canStart,
      newBlocked: canStart && noRoom !== null ? { reason: noRoom } : null,
      openNew,
      canRetry,
      retry,
      askClose,
      closeOffered,
      applied: kids.applied,
    }),
    [
      isChild,
      offer,
      kids,
      canWrite,
      canStart,
      noRoom,
      openNew,
      canRetry,
      retry,
      askClose,
      closeOffered,
    ],
  );

  return (
    <Context.Provider value={value}>
      {children}
      <NewSubagentDialog
        open={newOpen}
        onOpenChange={setNewOpen}
        parent={threadId}
        offer={offer}
        tier={tier}
        initial={retrying?.draft}
        onOpened={capabilities.refresh}
        onCardChanged={capabilities.refresh}
        onWithdrawn={() => {
          setWithdrawn(true);
          capabilities.refresh();
        }}
        onStarted={(request, draft) => {
          const from = retrying?.request;
          setDrafts((d) => {
            const next = new Map(d).set(request, draft);
            if (from) next.delete(from);
            return next;
          });
          setLocal((l) => [...l, request]);
        }}
        returnTo={opener}
      />
      <CloseSessionDialog
        open={closeOpen}
        onOpenChange={setCloseOpen}
        card={closing}
        onClosed={(child, state, cancel) => {
          kids.apply(child, state, cancel);
          refreshRows();
        }}
        returnTo={opener}
      />
    </Context.Provider>
  );
}
