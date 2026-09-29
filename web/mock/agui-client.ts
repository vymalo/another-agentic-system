/** A minimal AG-UI client for the mock's tests: the run POST and the connect GET, as frames. */
import { readSse } from "../src/features/chat/lib/agui/sse";

export type Frame = { id?: number; event: Record<string, unknown> };

export const RELEASE_CHANNELS_URI = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";

export type RunInput = {
  threadId: string;
  runId: string;
  messages: { id: string; role: "user"; content: string }[];
  resume?: { interruptId: string; status: "resolved" | "cancelled"; payload?: unknown }[];
  forwardedProps?: Record<string, unknown>;
};

export const runInput = (input: RunInput) => ({
  state: {},
  tools: [],
  context: [],
  forwardedProps: {},
  ...input,
});

export function postRun(base: string, agentId: string, input: RunInput): Promise<Response> {
  return fetch(`${base}/agui/agents/${agentId}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify(runInput(input)),
  });
}

export function connect(
  base: string,
  threadId: string,
  opts: { lastEventId?: number; mode?: "run"; signal?: AbortSignal } = {},
): Promise<Response> {
  const query = opts.mode ? `?mode=${opts.mode}` : "";
  return fetch(`${base}/agui/threads/${threadId}/connect${query}`, {
    ...(opts.signal ? { signal: opts.signal } : {}),
    headers: {
      Accept: "text/event-stream",
      ...(opts.lastEventId !== undefined ? { "Last-Event-ID": String(opts.lastEventId) } : {}),
    },
  });
}

/** Frames of a response until it ends, or `until` accepts one (the stream is then abandoned). */
export async function frames(res: Response, until?: (frame: Frame) => boolean): Promise<Frame[]> {
  const out: Frame[] = [];
  if (!res.body) return out;
  for await (const f of readSse(res.body)) {
    const frame: Frame = {
      ...(f.id !== undefined ? { id: Number(f.id) } : {}),
      event: JSON.parse(f.data) as Record<string, unknown>,
    };
    out.push(frame);
    if (until?.(frame)) break;
  }
  return out;
}

export const isTerminal = (f: Frame) =>
  f.event.type === "RUN_FINISHED" || f.event.type === "RUN_ERROR";
