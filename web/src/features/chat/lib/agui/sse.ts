/**
 * A server-sent-events reader that keeps the `id:` line.
 *
 * AG-UI's HTTP + SSE binding tells consumers to ignore `id:`; the connect stream (our extension,
 * docs/api/agui.md) puts the log `seq` there so a client can resume with `Last-Event-ID`. The
 * reference client's `parseSSEStream` drops it, so this small reader keeps it.
 *
 * Follows the WHATWG parsing rules the orchestrator writes for: LF, CRLF or CR line ends, `:`
 * comment lines (keepalives), several `data:` lines joined by LF, `event:` and `retry:` ignored.
 * A block cut off by the end of the stream is not a frame (it was never dispatched).
 */
export type SseFrame = {
  /** The block's own `id:` line; not sticky, unlike a browser's last-event-id buffer. */
  id?: string;
  data: string;
};

export type SseOptions = {
  /**
   * How long the body may say nothing at all — no frame, no comment, not even part of a line — before
   * the reader lets go of it (ADR 0082, C3). The orchestrator writes a `: keepalive` comment at least
   * every 15 s (`docs/api/agui.md`), so a window of two of them is a stream that died without closing:
   * a proxy, a phone radio or a NAT entry dropped it, and `reader.read()` would wait for ever. Letting
   * go ends the generator as an ordinary end, which is what the caller reopens from its last `id:`.
   *
   * Absent: the reader waits for ever. A run's response is read this way, because it is left at
   * `RUN_STARTED` and a watchdog there could turn a slow accept into a second send.
   */
  silenceMs?: number;
};

export async function* readSse(
  body: ReadableStream<Uint8Array>,
  signal?: AbortSignal,
  options: SseOptions = {},
): AsyncGenerator<SseFrame> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const onAbort = () => void reader.cancel().catch(() => {});
  signal?.addEventListener("abort", onAbort, { once: true });
  const silenceMs = options.silenceMs;
  let timer: ReturnType<typeof setTimeout> | undefined;
  // Any bytes the body delivers are its heartbeat, a comment as much as a frame: the window is re-armed
  // after each read, and its expiry closes the reader, which resolves the read in flight as `done`.
  // A hidden tab counts no silence: the browser throttles its timers and may pause the connection, so
  // the quiet is not the stream's. Becoming visible restarts a whole window.
  const hidden = () => typeof document !== "undefined" && document.visibilityState === "hidden";
  const rearm = () => {
    if (silenceMs === undefined) return;
    clearTimeout(timer);
    if (hidden()) return;
    timer = setTimeout(() => void reader.cancel().catch(() => {}), silenceMs);
  };
  const onVisibility = () => rearm();
  if (silenceMs !== undefined && typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibility);
  }
  let buffer = "";
  let id: string | undefined;
  let data: string[] = [];
  let skipLf = false; // a CR at the end of a chunk may be the first half of a CRLF

  const line = (text: string): SseFrame | undefined => {
    if (text === "") {
      const frame =
        data.length > 0
          ? { ...(id !== undefined ? { id } : {}), data: data.join("\n") }
          : undefined;
      id = undefined;
      data = [];
      return frame;
    }
    if (text.startsWith(":")) return undefined;
    const colon = text.indexOf(":");
    const field = colon === -1 ? text : text.slice(0, colon);
    let value = colon === -1 ? "" : text.slice(colon + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "data") data.push(value);
    else if (field === "id" && !value.includes("\0")) id = value;
    return undefined;
  };

  try {
    rearm();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return;
      rearm();
      let chunk = decoder.decode(value, { stream: true });
      if (skipLf && chunk.startsWith("\n")) chunk = chunk.slice(1);
      skipLf = chunk.endsWith("\r");
      buffer += chunk;
      let start = 0;
      for (let i = 0; i < buffer.length; i++) {
        const c = buffer[i];
        if (c !== "\n" && c !== "\r") continue;
        const frame = line(buffer.slice(start, i));
        if (c === "\r" && buffer[i + 1] === "\n") i++;
        start = i + 1;
        if (frame) yield frame;
      }
      buffer = buffer.slice(start);
    }
  } finally {
    clearTimeout(timer);
    if (typeof document !== "undefined")
      document.removeEventListener("visibilitychange", onVisibility);
    signal?.removeEventListener("abort", onAbort);
    await reader.cancel().catch(() => {});
  }
}
