import { useEffect, useRef, useState } from "react";
import { type ApiEvent, EVENT_KINDS } from "@/lib/api/types";

export type Connection = "connecting" | "open" | "reconnecting";

/** The subset of `EventSource` this hook uses, so tests can inject a fake. */
export type EventSourceLike = {
  readonly readyState: number;
  onopen: ((ev: Event) => void) | null;
  onerror: ((ev: Event) => void) | null;
  addEventListener(type: string, listener: (ev: MessageEvent<string>) => void): void;
  close(): void;
};
export type EventSourceFactory = (url: string) => EventSourceLike;

const CLOSED = 2;
const defaultFactory: EventSourceFactory = (url) => new EventSource(url);

const isDataFrame = (ev: Event): boolean => typeof (ev as MessageEvent).data === "string";

export const backoffMs = (attempt: number): number => Math.min(30_000, 1000 * 2 ** attempt);

type Options = {
  /** False closes the stream (e.g. a finished thread that is fully loaded). */
  enabled: boolean;
  onEvents: (events: ApiEvent[]) => void;
  factory?: EventSourceFactory;
};

/**
 * Follows `GET /api/threads/{id}/stream`.
 *
 * The server sends named SSE events (`event: <kind>`), so `onmessage` never fires: a listener
 * is registered for every kind in the contract. While the browser is retrying by itself it also
 * sends `Last-Event-ID`; if it gives up (non-200: 401/404/5xx) the stream is recreated with
 * exponential backoff, without a `Last-Event-ID`, which makes the server replay from seq 1 and
 * the reducer dedupe.
 */
export function useEventStream(
  threadId: string,
  { enabled, onEvents, factory = defaultFactory }: Options,
): Connection {
  const [connection, setConnection] = useState<Connection>("connecting");
  const onEventsRef = useRef(onEvents);
  onEventsRef.current = onEvents;
  const factoryRef = useRef(factory);
  factoryRef.current = factory;

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let source: EventSourceLike | undefined;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let attempt = 0;
    let buffer: ApiEvent[] = [];
    let flushScheduled = false;

    const flush = () => {
      flushScheduled = false;
      if (disposed || buffer.length === 0) return;
      const batch = buffer;
      buffer = [];
      onEventsRef.current(batch);
    };

    const connect = () => {
      const es = factoryRef.current(`/api/threads/${threadId}/stream`);
      source = es;
      es.onopen = () => {
        attempt = 0;
        setConnection("open");
      };
      for (const kind of EVENT_KINDS) {
        es.addEventListener(kind, (ev) => {
          // `error` is both a contract event kind and EventSource's own connection-error event
          // type: a plain Event is a connection error, only a MessageEvent carries our data.
          if (typeof ev.data !== "string") return;
          try {
            buffer.push(JSON.parse(ev.data) as ApiEvent);
          } catch {
            console.warn(`Dropping unparseable ${kind} frame`);
            return;
          }
          if (!flushScheduled) {
            flushScheduled = true;
            queueMicrotask(flush);
          }
        });
      }
      es.onerror = (ev) => {
        if (disposed) return;
        if (isDataFrame(ev)) return; // a server-sent `event: error` frame, not a connection error
        setConnection("reconnecting");
        if (es.readyState === CLOSED) {
          es.close();
          retry = setTimeout(connect, backoffMs(attempt++));
        }
        // otherwise the browser is retrying on its own (CONNECTING) and will send Last-Event-ID
      };
    };

    setConnection("connecting");
    connect();
    return () => {
      disposed = true;
      clearTimeout(retry);
      source?.close();
    };
  }, [threadId, enabled]);

  return connection;
}
