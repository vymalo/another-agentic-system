import { readFileSync } from "node:fs";
import path from "node:path";

/**
 * Test support: the AG-UI goldens of docs/api/examples/agui as frames and as a fake `fetch` that
 * plays the orchestrator's `connect` and run routes. Not part of the app bundle.
 */
export const GOLDEN_DIR = path.resolve(
  import.meta.dirname,
  "../../../../../../docs/api/examples/agui",
);

export const THREAD_ID = "22222222-2222-4222-8222-222222222222";

export type GoldenFrame = { id?: number; event: Record<string, unknown> };

export function loadGolden(name: string, threadId = THREAD_ID): GoldenFrame[] {
  const text = readFileSync(path.join(GOLDEN_DIR, `${name}.agui.json`), "utf8").replaceAll(
    "<thread-id>",
    threadId,
  );
  return JSON.parse(text) as GoldenFrame[];
}

/** One SSE frame as the orchestrator writes it. */
export const frameText = ({ id, event }: GoldenFrame): string =>
  `${id === undefined ? "" : `id: ${id}\n`}data: ${JSON.stringify(event)}\n\n`;

const encoder = new TextEncoder();

/** A response body the test writes to, and can cut. */
export class LiveStream {
  private controller!: ReadableStreamDefaultController<Uint8Array>;
  readonly body = new ReadableStream<Uint8Array>({
    start: (c) => {
      this.controller = c;
    },
    cancel: () => {
      this.cancelled = true;
      this.closed = true;
    },
  });
  closed = false;
  /** The reader let go of the body (the response was released). */
  cancelled = false;

  write(text: string) {
    if (!this.closed) this.controller.enqueue(encoder.encode(text));
  }

  frames(frames: GoldenFrame[]) {
    for (const f of frames) this.write(frameText(f));
  }

  /** The connection drops: the reader sees the end of the body. */
  cut() {
    if (this.closed) return;
    this.closed = true;
    this.controller.close();
  }
}

export const sse = (body: ReadableStream<Uint8Array>, status = 200) =>
  new Response(body, { status, headers: { "content-type": "text/event-stream" } });

export const problem = (status: number, title: string, detail?: string) =>
  new Response(JSON.stringify({ title, status, ...(detail ? { detail } : {}) }), {
    status,
    headers: { "content-type": "application/problem+json" },
  });

export type Call = { method: string; path: string; lastEventId?: string; body?: unknown };

/** A `fetch` for the routes ThreadAgent uses; `handler` answers each call. */
export function fakeFetch(
  handler: (call: Call, request: Request) => Response | Promise<Response>,
): { fetch: typeof fetch; calls: Call[] } {
  const calls: Call[] = [];
  const impl = async (input: RequestInfo | URL, init?: RequestInit) => {
    const request =
      input instanceof Request ? input : new Request(new URL(String(input), "http://x"), init);
    const url = new URL(request.url);
    const text = request.method === "POST" ? await request.clone().text() : "";
    const call: Call = {
      method: request.method,
      path: url.pathname,
      ...(request.headers.get("last-event-id") !== null
        ? { lastEventId: request.headers.get("last-event-id") as string }
        : {}),
      ...(text ? { body: JSON.parse(text) as unknown } : {}),
    };
    calls.push(call);
    return handler(call, request);
  };
  return { fetch: impl as typeof fetch, calls };
}
