import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { useSearchConfig } from "@/features/chat/hooks/use-ui-config";
import { dedupe, readAnswer } from "@/features/search/lib/answer";
import { Latest } from "@/features/search/lib/latest";
import {
  clip,
  isBlank,
  optionsOf,
  type SearchOption,
  step,
  wordCount,
} from "@/features/search/lib/options";
import { retryAfterSeconds } from "@/features/search/lib/retry";
import * as text from "@/features/search/lib/text";
import { api } from "@/lib/api/client";
import type { ApiThreadHit } from "@/lib/api/types";

/** The wait after the last key before a request goes out. */
export const DEBOUNCE_MS = 250;
/** Threads per page. */
const PAGE = 20;
/** How many times in a row a 429 is tried again by itself before the person is asked to. */
export const MAX_AUTO_RETRIES = 3;

/** The answer to one query: what was searched for, the terms the server read, and the threads found so far. */
export type SearchPage = {
  query: string;
  terms: string[];
  results: ApiThreadHit[];
  next?: string;
};

export type SearchStatus = "idle" | "searching" | "ready" | "error" | "limited" | "busy";

/** The request that failed or was limited: Retry sends exactly this again (the same text, the same cursor). */
type Pending = { query: string; cursor?: string };

type State = {
  status: SearchStatus;
  page: SearchPage | null;
  /** The wait a 429 asked for, in seconds (the box says "shortly" and shows no number). */
  seconds: number;
  more: boolean;
  pending: Pending | null;
  /** A 429 that was tried again {@link MAX_AUTO_RETRIES} times: only the person's Retry goes on. */
  exhausted: boolean;
};

const IDLE: State = {
  status: "idle",
  page: null,
  seconds: 0,
  more: false,
  pending: null,
  exhausted: false,
};

export type ThreadSearch = {
  /** The server serves the search (`search` of `GET /api/config`): without it there is no box. */
  enabled: boolean;
  maxQueryLength: number;
  value: string;
  setValue: (value: string) => void;
  /** The box holds something: the results take the rail's place. */
  active: boolean;
  status: SearchStatus;
  /** The last answer, kept (and dimmed) while the next is being fetched. */
  page: SearchPage | null;
  /** Seconds a 429 asked to wait. */
  seconds: number;
  /** A 429 that stopped being tried again by itself. */
  exhausted: boolean;
  loadingMore: boolean;
  /** The hits of the page that is drawn, in order. */
  options: SearchOption[];
  /**
   * The listbox is drawn for the text in the box and holds hits: only then do `aria-expanded`, `aria-controls`,
   * `aria-activedescendant`, the arrows and Enter mean anything.
   */
  listed: boolean;
  /** "Show more chats" was refused or limited: the hits stay, the alert is under them. */
  failedMore: boolean;
  /** "Show more chats" is offered: the page drawn is this text's, it is settled and it has a next page. */
  canMore: boolean;
  listId: string;
  activeIndex: number;
  activeId: string | undefined;
  move: (delta: 1 | -1) => void;
  retry: () => void;
  more: () => void;
  clear: () => void;
  /** The words a screen reader is told when the results change. */
  announcement: string;
  /** The person typed more words than the server uses. */
  truncated: number | null;
};

/**
 * The search of a person's own threads (ADR 0068, docs/api/thread-search-v1.md "Client recipe"). The text goes in the body of a POST
 * and nowhere else: not in a URL, not in storage, not in a log. A request goes out 250 ms after the last key; a change of the text
 * aborts the request in flight at once and drops its answer (an `AbortController` each, and a counter for an answer that came
 * anyway). A 429 is tried again after its `Retry-After`, at most {@link MAX_AUTO_RETRIES} times; a 503 is told and offers Retry;
 * a failed "Show more" keeps the hits and Retry sends the same page again. An answer that is not the contract's shape is an error.
 */
export function useThreadSearch(): ThreadSearch {
  const config = useSearchConfig();
  const maxQueryLength = config?.maxQueryLength ?? 200;
  const maxTerms = config?.maxTerms ?? 8;
  const [value, setValueState] = useState("");
  const [state, setState] = useState<State>(IDLE);
  const [activeIndex, setActiveIndex] = useState(-1);
  const latest = useRef(new Latest());
  const controller = useRef<AbortController | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const attempts = useRef(0);
  const prefix = useId();
  const listId = `${prefix}-results`;

  const stop = useCallback(() => {
    clearTimeout(timer.current);
    controller.current?.abort();
    controller.current = null;
    latest.current.cancel();
    attempts.current = 0;
  }, []);

  const run = useCallback(
    async (query: string, cursor?: string) => {
      const appending = cursor !== undefined;
      // a page of an earlier text never touches the timer of a newer one
      if (!appending) clearTimeout(timer.current);
      controller.current?.abort();
      const ctl = new AbortController();
      controller.current = ctl;
      const n = latest.current.next();
      const request: Pending = { query, ...(appending ? { cursor } : {}) };
      const failed = (status: SearchStatus, extra: Partial<State> = {}) =>
        setState((s) => ({ ...s, status, more: false, pending: request, ...extra }));
      setState((s) => ({
        ...s,
        status: appending ? "ready" : "searching",
        more: appending,
        pending: null,
        exhausted: false,
      }));
      try {
        const { data, response } = await api.POST("/api/search/threads", {
          body: {
            q: clip(query, maxQueryLength),
            limit: PAGE,
            archived: "include",
            ...(appending ? { cursor } : {}),
          },
          signal: ctl.signal,
        });
        if (!latest.current.is(n)) return;
        if (data !== undefined) {
          const answer = readAnswer(data);
          if (!answer) return failed("error");
          attempts.current = 0;
          setState((s) => ({
            status: "ready",
            seconds: 0,
            more: false,
            pending: null,
            exhausted: false,
            page: {
              query,
              terms: answer.terms,
              results:
                appending && s.page
                  ? dedupe([...s.page.results, ...answer.results])
                  : answer.results,
              ...(answer.next !== undefined ? { next: answer.next } : {}),
            },
          }));
          if (!appending) setActiveIndex(-1);
          return;
        }
        if (response.status === 429) {
          const seconds = retryAfterSeconds(response.headers.get("Retry-After"), Date.now());
          attempts.current += 1;
          if (attempts.current > MAX_AUTO_RETRIES)
            return failed("limited", { seconds, exhausted: true });
          failed("limited", { seconds });
          timer.current = setTimeout(() => void run(query, cursor), seconds * 1000);
          return;
        }
        failed(response.status === 503 ? "busy" : "error");
      } catch {
        // an aborted request is a newer one's business; anything else is an error to retry
        if (!latest.current.is(n)) return;
        failed("error");
      }
    },
    [maxQueryLength],
  );

  const setValue = useCallback(
    (next: string) => {
      setValueState(next);
      // the request in flight is for the text that is gone: aborted now, its answer dropped
      stop();
      setActiveIndex(-1);
      if (isBlank(next)) {
        setState(IDLE);
        return;
      }
      // the previous results stay (dimmed) until the answer for this text is in
      setState((s) => ({
        ...s,
        status: "searching",
        more: false,
        pending: null,
        exhausted: false,
      }));
      timer.current = setTimeout(() => void run(next), DEBOUNCE_MS);
    },
    [run, stop],
  );

  useEffect(() => stop, [stop]);

  const active = !isBlank(value);
  const page = state.page;
  const current = page !== null && active && page.query === value;
  const hits = current && page.results.length > 0;
  const failedMore =
    hits &&
    state.pending?.cursor !== undefined &&
    state.status !== "ready" &&
    state.status !== "searching";
  const listed = hits && (state.status === "ready" || failedMore);
  const canMore = current && state.status === "ready" && page.next !== undefined;

  const options = useMemo(() => (page ? optionsOf(page.results, listId) : []), [page, listId]);
  const move = useCallback(
    (delta: 1 | -1) => {
      if (listed) setActiveIndex((i) => step(i, delta, options.length));
    },
    [listed, options.length],
  );
  const retry = useCallback(() => {
    attempts.current = 0;
    const again = state.pending ?? { query: value };
    void run(again.query, again.cursor);
  }, [run, state.pending, value]);
  const more = useCallback(() => {
    const next = page?.next;
    if (page && next !== undefined && current && state.status === "ready")
      void run(page.query, next);
  }, [run, page, current, state.status]);
  const clear = useCallback(() => setValue(""), [setValue]);

  const announcement =
    state.status !== "ready" || !current
      ? ""
      : page.terms.length === 0
        ? text.NO_TERMS
        : page.results.length === 0
          ? text.noMatch(page.query.trim())
          : text.found(page.results.length, page.next !== undefined);
  const truncated =
    current && page.terms.length === maxTerms && wordCount(value) > maxTerms ? maxTerms : null;

  return {
    enabled: config !== undefined,
    maxQueryLength,
    value,
    setValue,
    active,
    status: state.status,
    page,
    seconds: state.seconds,
    exhausted: state.exhausted,
    loadingMore: state.more,
    options,
    listed,
    failedMore,
    canMore,
    listId,
    activeIndex,
    activeId: listed ? options[activeIndex]?.id : undefined,
    move,
    retry,
    more,
    clear,
    announcement,
    truncated,
  };
}
