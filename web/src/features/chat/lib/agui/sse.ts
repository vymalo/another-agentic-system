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

export async function* readSse(
  body: ReadableStream<Uint8Array>,
  signal?: AbortSignal,
): AsyncGenerator<SseFrame> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const onAbort = () => void reader.cancel().catch(() => {});
  signal?.addEventListener("abort", onAbort, { once: true });
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
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return;
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
    signal?.removeEventListener("abort", onAbort);
    await reader.cancel().catch(() => {});
  }
}
