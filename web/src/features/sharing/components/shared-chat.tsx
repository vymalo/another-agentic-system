"use client";

import { AssistantRuntimeProvider } from "@assistant-ui/react";
import { CheckIcon, CopyIcon, EyeIcon, RotateCcwIcon } from "lucide-react";
import { useMemo, useRef } from "react";
import { Thread } from "@/components/assistant-ui/elements/thread.aui";
import { PandaMark } from "@/components/brand/panda-mark";
import { Hint } from "@/components/hint";
import { InlineStatus, LoadingStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import { DataUIs } from "@/features/chat/components/data-uis";
import { DeliveryProvider } from "@/features/chat/components/delivery-note";
import { LiveDraftsProvider } from "@/features/chat/components/live-drafts";
import { LiveRuns } from "@/features/chat/components/live-runs";
import { StateBadge } from "@/features/chat/components/state-badge";
import { SurfaceHostProvider } from "@/features/chat/components/surface/surface-host";
import { ThreadDescription } from "@/features/chat/components/thread-description";
import { ThreadViewProvider } from "@/features/chat/components/thread-view";
import { useChatRuntime } from "@/features/chat/hooks/use-chat-runtime";
import type { Target } from "@/features/chat/lib/agui/thread-agent";
import { PanelToggle } from "@/features/panel/components/panel-toggle";
import { ThreadPanel } from "@/features/panel/components/thread-panel";
import { PanelProvider } from "@/features/panel/hooks/use-panel";
import type { ThreadsView } from "@/features/threads/hooks/use-threads";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import type { ApiSharedThread } from "@/lib/api/types";
import { useSharedThread } from "../hooks/use-shared-thread";
import type { ShareSource } from "../lib/sharing";
import { LinkDoesNotWork } from "./link-does-not-work";

/** Why a surface's buttons are off (`SurfaceHost.readOnly`): said in words under the surface. */
const READ_ONLY = "Read only: this is a shared conversation.";

/** A reader has no thread list. */
const NO_THREADS: ThreadsView = {
  threads: [],
  loading: false,
  error: null,
  hasMore: false,
  loadMore: () => {},
  refresh: () => {},
};
const NOTHING = () => {};

/**
 * The page of a share link, `/s/<token>` (ADR 0040, section 12). It reads the link (`useSharedThread`:
 * signed in first, then as anybody, else to sign in) and shows the neutral page for a link that does not
 * work, whatever the reason. A link that works is the conversation, read-only.
 */
export function SharedChat({ token }: { token: string }) {
  const { view, retry } = useSharedThread(token);
  switch (view.status) {
    case "gone":
      return <LinkDoesNotWork />;
    case "error":
      return (
        <main className="mx-auto my-12 w-full max-w-3xl px-4">
          <InlineStatus
            tone="error"
            role="alert"
            action={{ label: "Retry", icon: RotateCcwIcon, onClick: retry }}
          >
            {view.message}
          </InlineStatus>
        </main>
      );
    case "ready":
      return <SharedThreadView thread={view.thread} source={view.source} />;
    default:
      // loading, on the way to the thread (the owner) or on the way to sign in
      return (
        <main className="mx-auto my-12 w-full max-w-3xl px-4">
          <LoadingStatus>Opening the shared conversation…</LoadingStatus>
        </main>
      );
  }
}

/** The link as the reader's browser has it: what **Copy link** puts on the clipboard. */
function CopyLink() {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  return (
    <>
      <Hint label={isCopied ? "Copied" : "Copy link"}>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-9 rounded-full text-muted-foreground hover:text-foreground"
          aria-label="Copy link"
          onClick={() => copyToClipboard(window.location.href)}
        >
          {isCopied ? <CheckIcon aria-hidden="true" /> : <CopyIcon aria-hidden="true" />}
        </Button>
      </Hint>
      <span role="status" className="sr-only">
        {isCopied ? "Link copied." : ""}
      </span>
    </>
  );
}

/**
 * The shared conversation: the thread's own components in a mode that cannot act. The same
 * runtime, transcript, steps, cards and surfaces as the owner's chat (`ThreadAgent` follows the
 * link's route), and none of what acts: no composer, no sidebar, no menu, no fork or edit, no
 * rename, no export; the surfaces' buttons are off. Only **Copy link** and the details panel are left.
 */
function SharedThreadView({ thread, source }: { thread: ApiSharedThread; source: ShareSource }) {
  const target = useMemo<Target>(
    () => ({ agentId: thread.target.agentId, release: null }),
    [thread.target.agentId],
  );
  const chat = useChatRuntime({
    threadId: thread.id,
    target,
    threads: NO_THREADS,
    threadLastSeq: thread.lastSeq,
    notFound: false,
    onSendFailed: NOTHING,
    onSending: NOTHING,
    source,
  });
  const { snapshot, agent, runtime, loaded, revealed } = chat;
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const state = snapshot.state ?? thread.state;
  const title = snapshot.title ?? thread.title;
  const view = useMemo(
    () => ({
      state,
      // the question an agent asked is the owner's to answer, not the reader's
      waiting: false,
      agentId: thread.target.agentId,
      catalogVersion: snapshot.uiCatalog?.version,
    }),
    [state, thread.target.agentId, snapshot.uiCatalog?.version],
  );

  // the stream's reconnect met the 404: the link was taken down, or replaced, or narrowed
  if (snapshot.notFound) return <LinkDoesNotWork />;

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <SurfaceHostProvider
        agent={agent}
        readOnly={READ_ONLY}
        state={state}
        composerRef={composerRef}
        onRejected={NOTHING}
      >
        <DataUIs />
        <LiveRuns agent={agent} runtime={runtime} />
        <ThreadViewProvider value={view}>
          <PanelProvider>
            <div className="flex h-dvh overflow-hidden">
              <main className="flex min-h-0 min-w-0 flex-1 flex-col">
                <header className="flex h-14 shrink-0 items-center gap-2 px-3 md:px-4">
                  <PandaMark size={28} />
                  <h1
                    className="min-w-0 flex-1 truncate text-[0.9375rem] text-foreground"
                    title={title}
                  >
                    {title}
                  </h1>
                  <div className="flex shrink-0 items-center gap-1">
                    <StateBadge state={state} needsAnswer={false} />
                    <PanelToggle />
                    <CopyLink />
                  </div>
                </header>
                <div className="mx-auto w-full max-w-3xl px-4 pb-2 md:px-6">
                  <p
                    data-slot="shared-banner"
                    className="flex items-center gap-2.5 rounded-3xl border border-input bg-muted px-4 py-2.5 text-sm text-foreground"
                  >
                    <EyeIcon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
                    Shared conversation, read only
                  </p>
                  {thread.description ? (
                    <div className="pt-2">
                      <ThreadDescription key={thread.id} text={thread.description} />
                    </div>
                  ) : null}
                </div>
                <DeliveryProvider agent={thread.target.agentId} steers={null}>
                  <LiveDraftsProvider agent={agent}>
                    <Thread loading={!revealed} empty={loaded && snapshot.lastSeq === 0} />
                  </LiveDraftsProvider>
                </DeliveryProvider>
              </main>
              <ThreadPanel />
            </div>
          </PanelProvider>
        </ThreadViewProvider>
      </SurfaceHostProvider>
    </AssistantRuntimeProvider>
  );
}
