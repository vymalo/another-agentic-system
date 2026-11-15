import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { useSearchConfig } from "@/features/chat/hooks/use-ui-config";
import { Latest } from "@/features/search/lib/latest";
import {
  clip,
  isBlank,
  optionsOf,
  type SearchOption,
  step,
  wordCount,
} from "@/features/search/lib/options";
import * as text from "@/features/search/lib/text";
import { api } from "@/lib/api/client";
import type { ApiThreadHit } from "@/lib/api/types";

/** The wait after the last key before a request goes out. */
export const DEBOUNCE_MS = 250;
/** Threads per page. */
const PAGE = 20;
/** The longest wait the web honours for a `Retry-After`, in seconds. */
const MAX_WAIT_S = 30;

/** The answer to one query: what was searched for, the terms the server read, and the threads found so far. */
export type SearchPage = {
  query: string;
  terms: string[];
  results: ApiThreadHit[];
  next?: string;
};

export type SearchStatus = "idle" | "searching" | "ready" | "error" | "limited" | "busy";

type State = { status: SearchStatus; page: SearchPage | null; seconds: number; more: boolean };

const IDLE: State = { status: "idle", page: null, seconds: 0, more: false };

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
  /** Seconds before an automatic retry after a 429. */
  seconds: number;
  loadingMore: boolean;
  options: SearchOption[];
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
 * and nowhere else: not in a URL, not in storage, not in a log. A request goes out 250 ms after the last key; each has an
 * `AbortController`, and an answer that is not for the latest request is dropped even if the abort came too late. A 429 is tried again
 * after its `Retry-After`; a 503 is told and offers Retry.
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
  const prefix = useId();
  const listId = `${prefix}-results`;

  const stop = useCallback(() => {
    clearTimeout(timer.current);
    controller.current?.abort();
    controller.current = null;
    latest.current.cancel();
  }, []);

  const run = useCallback(
    async (query: string, cursor?: string) => {
      clearTimeout(timer.current);
      controller.current?.abort();
      const ctl = new AbortController();
      controller.current = ctl;
      const n = latest.current.next();
      const appending = cursor !== undefined;
      setState((s) => ({ ...s, status: appending ? s.status : "searching", more: appending }));
      try {
        const { data, response } = await api.POST("/api/search/threads", {
          body: {
            q: clip(query, maxQueryLength),
            limit: PAGE,
            archived: "include",
            ...(cursor !== undefined ? { cursor } : {}),
          },
          signal: ctl.signal,
        });
        if (!latest.current.is(n)) return;
        if (data) {
          setState((s) => ({
            status: "ready",
            seconds: 0,
            more: false,
            page: {
              query,
              terms: data.query.terms,
              results: appending && s.page ? [...s.page.results, ...data.results] : data.results,
              ...(data.next !== undefined ? { next: data.next } : {}),
            },
          }));
          if (!appending) setActiveIndex(-1);
          return;
        }
        if (response.status === 429) {
          const asked = Number.parseInt(response.headers.get("Retry-After") ?? "", 10);
          const seconds = Math.min(MAX_WAIT_S, Number.isFinite(asked) && asked > 0 ? asked : 1);
          setState((s) => ({ ...s, status: "limited", seconds, more: false }));
          timer.current = setTimeout(() => void run(query, cursor), seconds * 1000);
          return;
        }
        setState((s) => ({
          ...s,
          status: response.status === 503 ? "busy" : "error",
          more: false,
        }));
      } catch {
        // an aborted request is a newer one's business; anything else is an error to retry
        if (!latest.current.is(n)) return;
        setState((s) => ({ ...s, status: "error", more: false }));
      }
    },
    [maxQueryLength],
  );

  const setValue = useCallback(
    (next: string) => {
      setValueState(next);
      clearTimeout(timer.current);
      if (isBlank(next)) {
        stop();
        setState(IDLE);
        setActiveIndex(-1);
        return;
      }
      // the previous results stay (dimmed) until the answer for this text is in
      setState((s) => ({ ...s, status: "searching", more: false }));
      timer.current = setTimeout(() => void run(next), DEBOUNCE_MS);
    },
    [run, stop],
  );

  useEffect(() => stop, [stop]);

  const options = useMemo(
    () => (state.page ? optionsOf(state.page.results, listId) : []),
    [state.page, listId],
  );
  const move = useCallback(
    (delta: 1 | -1) => setActiveIndex((i) => step(i, delta, options.length)),
    [options.length],
  );
  const retry = useCallback(() => void run(value), [run, value]);
  const more = useCallback(() => {
    const next = state.page?.next;
    if (state.page && next !== undefined) void run(state.page.query, next);
  }, [run, state.page]);
  const clear = useCallback(() => setValue(""), [setValue]);

  const active = !isBlank(value);
  const page = state.page;
  const announcement =
    state.status !== "ready" || !page
      ? ""
      : page.terms.length === 0
        ? text.NO_TERMS
        : page.results.length === 0
          ? text.noMatch(page.query.trim())
          : text.found(page.results.length, page.next !== undefined);
  const truncated =
    active && page?.terms.length === maxTerms && wordCount(value) > maxTerms ? maxTerms : null;

  return {
    enabled: config !== undefined,
    maxQueryLength,
    value,
    setValue,
    active,
    status: state.status,
    page,
    seconds: state.seconds,
    loadingMore: state.more,
    options,
    listId,
    activeIndex,
    activeId: options[activeIndex]?.id,
    move,
    retry,
    more,
    clear,
    announcement,
    truncated,
  };
}
