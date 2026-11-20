"use client";

import { RotateCcwIcon, SearchIcon, XIcon } from "lucide-react";
import {
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { Hint } from "@/components/hint";
import { InlineStatus, LoadingStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import type { ThreadSearch } from "@/features/search/hooks/use-thread-search";
import type { SearchOption } from "@/features/search/lib/options";
import { runsOf } from "@/features/search/lib/runs";
import {
  focusDone,
  focusPending,
  isApple,
  takeReturnTarget,
  useSearchFocusRequests,
} from "@/features/search/lib/shortcut";
import * as text from "@/features/search/lib/text";
import type { ApiSearchSnippet, ApiThreadHit } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/** Frames a request for the box waits for it to be shown (a sidebar that is opening, a sheet that is mounting). */
const FOCUS_FRAMES = 30;

/** Whether the element is on screen: an engine that cannot say is taken to show it. */
function isShown(el: HTMLElement): boolean {
  return typeof el.checkVisibility === "function"
    ? el.checkVisibility({ visibilityProperty: true })
    : true;
}

/**
 * The search box under "New chat" (ADR 0068): a combobox that controls the listbox of results, which takes the rail's place while
 * the box holds a query. Keys: Down and Up walk the hits (`aria-activedescendant`, the focus stays in the box), Enter opens the one
 * walked to (the first when none is), the first Escape clears the box and the next gives the focus back to where it was. Ctrl or ⌘
 * with K brings the focus here from anywhere (`useSearchShortcut`, `features/search/lib/shortcut.ts`).
 */
export function SearchBox({
  search,
  onOpen,
}: {
  search: ThreadSearch;
  onOpen: (option: SearchOption) => void;
}) {
  const input = useRef<HTMLInputElement | null>(null);
  const frame = useRef(0);
  const [apple, setApple] = useState(false);
  useEffect(() => setApple(isApple()), []);

  const focusIt = useCallback(() => {
    cancelAnimationFrame(frame.current);
    let left = FOCUS_FRAMES;
    const tick = () => {
      const el = input.current;
      if (el && isShown(el)) {
        el.focus();
        el.select();
        focusDone();
        return;
      }
      left -= 1;
      if (left > 0) frame.current = requestAnimationFrame(tick);
    };
    tick();
  }, []);
  useSearchFocusRequests(focusIt);
  // a sheet that opens because of the shortcut mounts this box after the request was made
  useEffect(() => {
    if (focusPending()) focusIt();
    return () => cancelAnimationFrame(frame.current);
  }, [focusIt]);

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    // Safari ends a composition with a keydown that still says it is composing (keyCode 229)
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    const { listed } = search;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      if (!listed) return;
      e.preventDefault();
      search.move(e.key === "ArrowDown" ? 1 : -1);
    } else if (e.key === "Enter") {
      if (!listed) return;
      e.preventDefault();
      const chosen = search.options[search.activeIndex] ?? search.options[0];
      if (chosen) onOpen(chosen);
    } else if (e.key === "Escape") {
      if (search.value !== "") {
        e.preventDefault();
        search.clear();
      } else {
        const back = takeReturnTarget();
        if (back) back.focus();
        else e.currentTarget.blur();
      }
    }
  };

  return (
    <search data-slot="search" className="relative mb-2 block px-1">
      <SearchIcon
        aria-hidden="true"
        className="pointer-events-none absolute start-4 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
      />
      <input
        ref={input}
        type="search"
        role="combobox"
        data-slot="search-input"
        aria-label={text.SEARCH_LABEL}
        aria-autocomplete="list"
        aria-expanded={search.listed}
        aria-controls={search.listed ? search.listId : undefined}
        aria-activedescendant={search.activeId}
        aria-keyshortcuts={apple ? text.KEYSHORTCUTS_APPLE : text.KEYSHORTCUTS}
        autoComplete="off"
        autoCapitalize="off"
        spellCheck={false}
        enterKeyHint="search"
        maxLength={search.maxQueryLength}
        placeholder={text.SEARCH_PLACEHOLDER}
        value={search.value}
        onChange={(e) => search.setValue(e.target.value)}
        onKeyDown={onKeyDown}
        className={cn(
          "h-10 w-full rounded-full border border-transparent bg-sidebar-accent/60 ps-9 pe-14 text-sm text-foreground",
          "placeholder:text-muted-foreground focus-visible:border-ring focus-visible:outline-none",
          "[&::-webkit-search-cancel-button]:hidden [&::-webkit-search-decoration]:hidden",
        )}
      />
      {search.value !== "" ? (
        <Hint label={text.CLEAR_LABEL} side="top">
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={text.CLEAR_LABEL}
            onClick={() => {
              search.clear();
              input.current?.focus();
            }}
            className="absolute end-2 top-1/2 size-7 -translate-y-1/2 rounded-full text-muted-foreground hover:text-foreground"
          >
            <XIcon aria-hidden="true" className="size-3.5" />
          </Button>
        </Hint>
      ) : (
        <span
          aria-hidden="true"
          className="pointer-events-none absolute end-4 top-1/2 hidden -translate-y-1/2 font-sans text-xs text-muted-foreground md:inline"
        >
          {apple ? text.SHORTCUT_APPLE : text.SHORTCUT_OTHER}
        </span>
      )}
      {search.truncated !== null ? (
        <p className="px-3 pt-1 text-xs text-muted-foreground">
          {text.onlyFirst(search.truncated)}
        </p>
      ) : null}
      <div role="status" aria-live="polite" className="sr-only">
        {search.announcement}
      </div>
    </search>
  );
}

/** A snippet as plain runs and `<mark>`s, with an ellipsis where the document goes on. Text nodes only: nothing is markup. */
function Snippet({ snippet }: { snippet: ApiSearchSnippet }) {
  return (
    <>
      {snippet.cutStart ? <span aria-hidden="true">{text.ELLIPSIS}</span> : null}
      {runsOf(snippet).map((run) =>
        run.mark ? (
          <mark key={run.at} className="rounded-xs bg-brand/20 px-px text-foreground">
            {run.text}
          </mark>
        ) : (
          <span key={run.at}>{run.text}</span>
        ),
      )}
      {snippet.cutEnd ? <span aria-hidden="true">{text.ELLIPSIS}</span> : null}
    </>
  );
}

const day = (at: string): string => {
  const date = new Date(at);
  return Number.isNaN(date.getTime())
    ? ""
    : date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
};

/** One hit: a link that is an option of the listbox, so that a middle click or a copied address still works. */
function HitOption({
  option,
  selected,
  onOpen,
}: {
  option: SearchOption;
  selected: boolean;
  onOpen: (option: SearchOption) => void;
}) {
  const { thread, hit } = option;
  const archived = thread.archived === true;
  const title = thread.title || "Untitled";
  const click = (e: MouseEvent<HTMLAnchorElement>) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) {
      return;
    }
    e.preventDefault();
    onOpen(option);
  };
  let body: ReactNode;
  if (hit.field === "title") {
    body = (
      <span className="flex min-w-0 items-baseline gap-2">
        <span className="line-clamp-2 min-w-0 flex-1 text-sm font-medium break-words">
          <Snippet snippet={hit.snippet} />
        </span>
        {archived ? (
          <span className="shrink-0 text-xs text-muted-foreground">{text.ARCHIVED}</span>
        ) : null}
      </span>
    );
  } else {
    const label =
      hit.field === "description"
        ? text.FIELD.description
        : [hit.role ? text.WHO[hit.role] : "", hit.at ? day(hit.at) : ""]
            .filter(Boolean)
            .join(" · ");
    body = (
      <>
        <span className="sr-only">
          {title}
          {archived ? ` (${text.ARCHIVED.toLowerCase()})` : ""}:{" "}
        </span>
        <span className="block text-xs text-muted-foreground">{label}</span>
        <span className="line-clamp-2 text-[0.8125rem] break-words text-foreground/90">
          <Snippet snippet={hit.snippet} />
        </span>
      </>
    );
  }
  return (
    <a
      role="option"
      id={option.id}
      href={option.href}
      aria-selected={selected}
      tabIndex={-1}
      data-slot="search-hit"
      data-field={hit.field}
      {...(hit.seq !== undefined ? { "data-hit-seq": hit.seq } : {})}
      // the focus stays in the box: a press on a hit must not take it
      onMouseDown={(e) => e.preventDefault()}
      onClick={click}
      className={cn(
        "block min-w-0 rounded-xl px-3 py-1.5 text-foreground no-underline transition-colors",
        "hover:bg-sidebar-accent/70 aria-selected:bg-sidebar-accent",
        hit.field !== "title" && "ms-3",
      )}
    >
      {body}
    </a>
  );
}

/** One thread that matched: its title (a hit, or a plain heading) and its hits, then how many more messages match. */
function Result({
  result,
  options,
  activeId,
  onOpen,
}: {
  result: ApiThreadHit;
  options: SearchOption[];
  activeId: string | undefined;
  onOpen: (option: SearchOption) => void;
}) {
  const hasTitle = result.hits.some((h) => h.field === "title");
  const shown = result.hits.filter((h) => h.field === "message").length;
  const more = result.messageHits - shown;
  return (
    <div role="presentation" data-slot="search-result" className="flex flex-col gap-px pb-2">
      {hasTitle ? null : (
        <div
          aria-hidden="true"
          className="flex items-baseline gap-2 px-3 pt-1 text-sm font-medium text-foreground"
        >
          <span className="min-w-0 flex-1 truncate">{result.thread.title || "Untitled"}</span>
          {result.thread.archived ? (
            <span className="shrink-0 text-xs font-normal text-muted-foreground">
              {text.ARCHIVED}
            </span>
          ) : null}
        </div>
      )}
      {options.map((option) => (
        <HitOption
          key={option.id}
          option={option}
          selected={option.id === activeId}
          onOpen={onOpen}
        />
      ))}
      {more > 0 ? (
        <div role="presentation" className="ms-6 px-3 text-xs text-muted-foreground">
          {text.moreMessages(more)}
        </div>
      ) : null}
    </div>
  );
}

/**
 * The results, in the rail's place while the box holds a query: searching (the last answer dimmed while the next is fetched),
 * the hits, no match, or an error, a rate limit or a busy server in plain words, each with a way to try again.
 */
export function SearchResults({
  search,
  onOpen,
}: {
  search: ThreadSearch;
  onOpen: (option: SearchOption) => void;
}) {
  const { page, status } = search;
  // the hit walked to is scrolled into view; the focus never leaves the box
  const { activeId } = search;
  useEffect(() => {
    if (activeId) document.getElementById(activeId)?.scrollIntoView({ block: "nearest" });
  }, [activeId]);

  // a failed "Show more": the hits already shown stay, and the alert is under them
  const alert =
    status === "error" || status === "busy" ? (
      <InlineStatus
        tone="error"
        role="alert"
        action={{ label: text.RETRY, icon: RotateCcwIcon, onClick: search.retry }}
      >
        {status === "busy" ? text.BUSY : text.ERROR}
      </InlineStatus>
    ) : status === "limited" ? (
      search.exhausted ? (
        <InlineStatus
          tone="error"
          role="alert"
          action={{ label: text.RETRY, icon: RotateCcwIcon, onClick: search.retry }}
        >
          {text.STILL_LIMITED}
        </InlineStatus>
      ) : (
        <InlineStatus
          tone="warning"
          role="status"
          action={{ label: text.RETRY_NOW, onClick: search.retry }}
        >
          {text.RATE_LIMITED}
        </InlineStatus>
      )
    ) : null;

  let body: ReactNode;
  if (alert && !search.failedMore) {
    body = alert;
  } else if (!page) {
    body = <LoadingStatus rows={3}>{text.SEARCHING}</LoadingStatus>;
  } else if (page.terms.length === 0) {
    body = <p className="px-3 text-sm text-muted-foreground">{text.NO_TERMS}</p>;
  } else if (page.results.length === 0) {
    body = (
      <p data-slot="search-empty" className="px-3 text-sm break-words text-muted-foreground">
        {text.noMatch(page.query.trim())}
      </p>
    );
  } else {
    let at = 0;
    body = (
      <>
        <div
          id={search.listId}
          role="listbox"
          aria-label={text.RESULTS_LABEL}
          className="flex flex-col"
        >
          {page.results.map((result) => {
            const options = search.options.slice(at, at + result.hits.length);
            at += result.hits.length;
            return (
              <Result
                key={result.thread.id}
                result={result}
                options={options}
                activeId={activeId}
                onOpen={onOpen}
              />
            );
          })}
        </div>
        {search.failedMore ? alert : null}
        {search.canMore || search.loadingMore ? (
          <Button
            type="button"
            variant="ghost"
            disabled={search.loadingMore}
            className="h-9 justify-start rounded-full px-3 text-muted-foreground"
            onClick={search.more}
          >
            {text.SHOW_MORE}
          </Button>
        ) : null}
      </>
    );
  }
  return (
    <div
      data-slot="search-results"
      aria-busy={status === "searching"}
      className={cn(
        "flex flex-col gap-1 px-1 transition-opacity",
        status === "searching" && page ? "opacity-60" : "opacity-100",
      )}
    >
      {body}
    </div>
  );
}
