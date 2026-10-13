"use client";

import {
  ChevronRightIcon,
  MenuIcon,
  PanelLeftCloseIcon,
  PanelLeftOpenIcon,
  RotateCcwIcon,
  SquarePenIcon,
  XIcon,
} from "lucide-react";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import {
  type ReactNode,
  type Ref,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { PandaMark } from "@/components/brand/panda-mark";
import { Hint } from "@/components/hint";
import { InlineStatus, LoadingStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetClose,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
import { useShowDescriptions } from "@/features/chat/hooks/use-ui-config";
import { useMe } from "@/features/me/hooks/use-me";
import { holds } from "@/features/me/lib/access";
import { AccountMenu } from "@/features/session/components/account-menu";
import { useThreadBranches } from "@/features/threads/components/branches-provider";
import { DeleteThreadDialog } from "@/features/threads/components/delete-thread-dialog";
import { type RowHandlers, ThreadRow } from "@/features/threads/components/thread-row";
import type { ArrangeAction, ThreadsView } from "@/features/threads/hooks/use-threads";
import { type Block, blocksOf, type Section, sectionsOf } from "@/features/threads/lib/rail";
import type { ApiThread } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/** A section of the list: its heading, and its blocks (a thread and the threads nested under it). */
function Blocks({
  blocks,
  section,
  ...row
}: {
  blocks: Block[];
  section: Section;
  mayDelete: boolean;
  showDescriptions: boolean;
  pathname: string;
  root: string | null;
  onArrange: RowHandlers["onArrange"];
  onDelete: RowHandlers["onDelete"];
}) {
  const one = (t: ApiThread, nested?: ReactNode) => (
    <ThreadRow
      key={t.id}
      thread={t}
      description={row.showDescriptions ? t.description : undefined}
      active={row.pathname === `/threads/${t.id}`}
      family={t.id === row.root}
      section={section}
      mayDelete={row.mayDelete}
      nested={nested}
      onArrange={row.onArrange}
      onDelete={row.onDelete}
    />
  );
  return (
    <ul className="flex flex-col gap-px">
      {blocks.map(({ root, children }) =>
        one(
          root,
          children.length > 0 ? (
            <ul
              data-slot="thread-children"
              className="ms-5 mt-px flex flex-col gap-px border-s border-border ps-1"
            >
              {children.map((c) => one(c))}
            </ul>
          ) : undefined,
        ),
      )}
    </ul>
  );
}

/** The archived chats: a heading that is the button that opens them, read when it opens (`archived=only`). */
function ArchivedSection({
  view,
  ...row
}: {
  view: ThreadsView["archived"];
  mayDelete: boolean;
  showDescriptions: boolean;
  pathname: string;
  root: string | null;
  onArrange: RowHandlers["onArrange"];
  onDelete: RowHandlers["onDelete"];
}) {
  const panel = useId();
  const blocks = useMemo(() => blocksOf(view.threads), [view.threads]);
  return (
    <div className="flex flex-col pb-3">
      <h2 className="text-xs font-medium text-muted-foreground">
        <button
          type="button"
          data-slot="archived-toggle"
          aria-expanded={view.open}
          aria-controls={panel}
          onClick={view.toggle}
          className="flex h-8 w-full items-center gap-1 rounded-full px-3 text-start hover:bg-sidebar-accent/70"
        >
          <ChevronRightIcon
            aria-hidden="true"
            className={cn("size-3.5 transition-transform", view.open && "rotate-90")}
          />
          Archived
        </button>
      </h2>
      <div id={panel} hidden={!view.open}>
        {!view.open ? null : view.error ? (
          <div className="px-1">
            <InlineStatus tone="error" role="alert">
              Could not load archived chats: {view.error}
            </InlineStatus>
          </div>
        ) : view.loading ? (
          <LoadingStatus rows={2} className="px-1 pt-1">
            Loading archived chats…
          </LoadingStatus>
        ) : blocks.length === 0 ? (
          <p className="px-3 py-1 text-sm text-muted-foreground">Nothing archived.</p>
        ) : (
          <Blocks blocks={blocks} section="archived" {...row} />
        )}
        {view.open && view.hasMore ? (
          <Button
            type="button"
            variant="ghost"
            className="h-9 justify-start rounded-full px-3 text-muted-foreground"
            onClick={view.loadMore}
          >
            Load older
          </Button>
        ) : null}
      </div>
    </div>
  );
}

/**
 * "New chat", then the person's own list (ADR 0042): Pinned, Chats and Archived, each in the order the
 * server keeps, a fork nested under its parent, and paging. A row's menu pins, archives, ejects or deletes it;
 * the dialog of a delete is here, beside the list, so that it outlives the menu that opened it. The account
 * menu is under it.
 */
function ThreadNav({ threads }: { threads: ThreadsView }) {
  const pathname = usePathname();
  const router = useRouter();
  // the open thread may be an edit, which the list leaves out: its conversation's first thread is the row
  const { root } = useThreadBranches();
  const showDescriptions = useShowDescriptions();
  const { me, status } = useMe();
  // the roles decide what is offered; the orchestrator decides what is allowed (ADR 0033). Unknown: offered.
  const mayDelete = me ? holds(me, "thread.delete") : status === "unknown";
  const blocks = useMemo(() => blocksOf(threads.threads), [threads.threads]);
  const { pinned, chats } = useMemo(() => sectionsOf(blocks), [blocks]);
  const newChat = useRef<HTMLAnchorElement | null>(null);
  const [deleting, setDeleting] = useState<{ thread: ApiThread; from: HTMLElement | null } | null>(
    null,
  );
  const { arrange, forget } = threads;
  // A row that moves to another section is a new element, and the focus that was on its button is gone with
  // the old one: when the list has been read again the button of the same thread takes it back (the toggle
  // of Archived when the row went there), unless the person has gone elsewhere in the meantime.
  const nav = useRef<HTMLElement | null>(null);
  const refocus = useRef<{ id: string; action: ArrangeAction } | null>(null);
  const [arranged, setArranged] = useState(0);
  const onArrange = useCallback(
    async (thread: ApiThread, action: ArrangeAction) => {
      await arrange(thread, action);
      refocus.current = { id: thread.id, action };
      setArranged((n) => n + 1);
    },
    [arrange],
  );
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs once per finished arrangement, on purpose
  useEffect(() => {
    const want = refocus.current;
    refocus.current = null;
    if (!want || !nav.current) return;
    const lost = document.activeElement === null || document.activeElement === document.body;
    if (!lost) return;
    const button = nav.current.querySelector<HTMLElement>(
      `[data-thread-id="${want.id}"] [data-slot="thread-menu-trigger"]`,
    );
    const toggle = nav.current.querySelector<HTMLElement>('[data-slot="archived-toggle"]');
    (button ?? (want.action === "archive" ? toggle : null) ?? newChat.current)?.focus();
  }, [arranged]);
  const onDelete = useCallback(
    (thread: ApiThread, from: HTMLElement | null) => setDeleting({ thread, from }),
    [],
  );
  const erased = useCallback(
    (id: string) => {
      forget(id);
      // the chat that is open is gone, and so is the one an open edit was made from: the page is the new chat
      if (pathname === `/threads/${id}` || root === id) router.replace("/");
      setDeleting(null);
    },
    [forget, pathname, root, router],
  );
  const row = { mayDelete, showDescriptions, pathname, root, onArrange, onDelete };
  return (
    <nav ref={nav} aria-label="Threads" className="flex flex-col gap-1 px-2 pb-4">
      <Link
        ref={newChat}
        href="/"
        className="mb-3 flex h-10 items-center gap-2.5 rounded-full px-3 text-sm font-medium text-foreground no-underline transition-colors hover:bg-sidebar-accent/70"
      >
        <SquarePenIcon aria-hidden="true" className="size-4 text-muted-foreground" />
        New chat
      </Link>
      {threads.notice ? (
        <div className="px-1">
          <InlineStatus
            tone="error"
            role="alert"
            action={{ label: "Dismiss", icon: XIcon, onClick: threads.dismissNotice }}
          >
            {threads.notice}
          </InlineStatus>
        </div>
      ) : null}
      {threads.error ? (
        <div className="px-1">
          <InlineStatus
            tone="error"
            action={{ label: "Retry", icon: RotateCcwIcon, onClick: threads.refresh }}
          >
            Could not load threads.
          </InlineStatus>
        </div>
      ) : threads.loading ? (
        <LoadingStatus className="px-1 pt-1">Loading threads…</LoadingStatus>
      ) : threads.threads.length === 0 ? (
        <p className="px-3 text-sm text-muted-foreground">Your chats will show up here.</p>
      ) : null}
      {pinned.length > 0 ? (
        <div className="flex flex-col pb-3">
          <h2 className="px-3 pb-1 text-xs font-medium text-muted-foreground">Pinned</h2>
          <Blocks blocks={pinned} section="pinned" {...row} />
        </div>
      ) : null}
      {chats.length > 0 ? (
        <div className="flex flex-col pb-3">
          <h2 className="px-3 pb-1 text-xs font-medium text-muted-foreground">Chats</h2>
          <Blocks blocks={chats} section="chats" {...row} />
        </div>
      ) : null}
      {threads.hasMore ? (
        <Button
          type="button"
          variant="ghost"
          className="h-9 justify-start rounded-full px-3 text-muted-foreground"
          onClick={threads.loadMore}
        >
          Load older
        </Button>
      ) : null}
      <ArchivedSection view={threads.archived} {...row} />
      {deleting ? (
        <DeleteThreadDialog
          key={deleting.thread.id}
          thread={deleting.thread}
          onClose={() => setDeleting(null)}
          onErased={erased}
          // the row's button when the chat stays; when it is gone, the way to a new one
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            const back = deleting.from?.isConnected ? deleting.from : newChat.current;
            back?.focus();
          }}
        />
      ) : null}
    </nav>
  );
}

/** The brand row of the sidebar, with the control that collapses it (a desktop). */
function SidebarTop({
  onCollapse,
  collapseRef,
  close,
}: {
  onCollapse?: () => void;
  collapseRef?: Ref<HTMLButtonElement>;
  close?: ReactNode;
}) {
  return (
    <div className="flex h-14 shrink-0 items-center justify-between ps-4 pe-2">
      <Link
        href="/"
        className="flex items-center gap-2 rounded-full text-[0.9375rem] font-semibold tracking-tight text-foreground no-underline"
      >
        <PandaMark size={28} />
        <span>
          another<span className="text-brand">·</span>agentic
        </span>
      </Link>
      {onCollapse ? (
        <Hint label="Close sidebar">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            ref={collapseRef}
            aria-label="Close sidebar"
            onClick={onCollapse}
            // colours only: `transition-all` would also transition the `visibility` this button
            // inherits from the sidebar, and the focus moved here on opening could land on its first
            // frame, still hidden
            className="size-9 rounded-full text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground"
          >
            <PanelLeftCloseIcon aria-hidden="true" className="size-4.5" />
          </Button>
        </Hint>
      ) : null}
      {close}
    </div>
  );
}

/**
 * Wide screens: a column beside the chat that the person can collapse (the shell remembers it).
 * Hidden on phones, where `ThreadsSheet` opens the same list.
 */
export function ThreadSidebar({
  threads,
  open,
  onCollapse,
  collapseRef,
}: {
  threads: ThreadsView;
  open: boolean;
  onCollapse: () => void;
  /** The "Close sidebar" button, which takes the focus when the sidebar opens. */
  collapseRef?: Ref<HTMLButtonElement>;
}) {
  return (
    <aside
      data-slot="sidebar"
      data-state={open ? "open" : "closed"}
      className={cn(
        "hidden min-h-0 shrink-0 overflow-hidden bg-sidebar transition-[width] duration-200 ease-out md:block dark:border-r",
        open ? "w-68" : "w-0 border-r-0",
      )}
    >
      <div
        className={cn("flex h-full w-68 flex-col", !open && "invisible")}
        aria-hidden={open ? undefined : true}
      >
        <SidebarTop onCollapse={onCollapse} collapseRef={collapseRef} />
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
          <ThreadNav threads={threads} />
        </div>
        <AccountMenu />
      </div>
    </aside>
  );
}

/** A desktop with the sidebar collapsed: the controls to open it again and to start a chat. */
export function SidebarOpeners({
  onOpen,
  openRef,
}: {
  onOpen: () => void;
  /** The "Open sidebar" button, which takes the focus when the sidebar closes. */
  openRef?: Ref<HTMLButtonElement>;
}) {
  return (
    <div className="hidden items-center gap-0.5 md:flex">
      <Hint label="Open sidebar">
        <Button
          type="button"
          variant="ghost"
          size="icon"
          ref={openRef}
          aria-label="Open sidebar"
          onClick={onOpen}
          className="size-9 rounded-full text-muted-foreground hover:text-foreground"
        >
          <PanelLeftOpenIcon aria-hidden="true" className="size-4.5" />
        </Button>
      </Hint>
      <Hint label="New chat">
        <Button
          asChild
          variant="ghost"
          size="icon"
          className="size-9 rounded-full text-muted-foreground hover:text-foreground"
        >
          <Link href="/" aria-label="New chat">
            <SquarePenIcon aria-hidden="true" className="size-4.5" />
          </Link>
        </Button>
      </Hint>
    </div>
  );
}

/**
 * Phones: a menu button ("Threads") that opens the list in a sheet from the left. The sheet
 * content is unmounted while closed, so only one "Threads" navigation exists at a time, and it
 * closes when the route changes (a thread was picked). Focus returns to the button on close.
 */
export function ThreadsSheet({ threads }: { threads: ThreadsView }) {
  const [open, setOpen] = useState(false);
  const pathname = usePathname();
  // biome-ignore lint/correctness/useExhaustiveDependencies: closing on navigation is the point
  useEffect(() => setOpen(false), [pathname]);

  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <Hint label="Threads">
        <SheetTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label="Threads"
            className="size-10 rounded-full text-muted-foreground hover:text-foreground md:hidden"
          >
            <MenuIcon aria-hidden="true" className="size-5" />
          </Button>
        </SheetTrigger>
      </Hint>
      <SheetContent
        side="left"
        showCloseButton={false}
        aria-describedby={undefined}
        className="w-[min(18rem,85vw)] gap-0 border-r-0 bg-sidebar p-0 shadow-xl data-[side=left]:w-[min(18rem,85vw)]"
      >
        <SheetHeader className="sr-only">
          <SheetTitle>Threads</SheetTitle>
        </SheetHeader>
        <SidebarTop
          close={
            <Hint label="Close">
              <SheetClose asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  aria-label="Close"
                  className="size-9 rounded-full text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
                >
                  <XIcon aria-hidden="true" className="size-4.5" />
                </Button>
              </SheetClose>
            </Hint>
          }
        />
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
          <ThreadNav threads={threads} />
        </div>
        <AccountMenu />
      </SheetContent>
    </Sheet>
  );
}
