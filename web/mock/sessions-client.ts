/** What the mock's tests of child sessions share: a server, the calls a person makes, and the log of a thread. */
import type { AddressInfo } from "node:net";
import type { components } from "../src/lib/api/schema";
import { createMockServer } from "./server";

export type Thread = components["schemas"]["Thread"];
export type LogEvent = components["schemas"]["Event"];

export type Mock = {
  base: string;
  /** A session of its own: its cookie, so that a test's switches reach no other test. */
  session: (config?: string) => Promise<Person>;
  close: () => Promise<void>;
};

let sessions = 0;
let ids = 0;

/** A thread id the mock accepts. */
export const newId = (): string => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`;
/** The id of a start request. */
export const newRequest = (): string =>
  `00000000-0000-4000-9000-${String(++ids).padStart(12, "0")}`;

export async function startMock(stepMs = 5): Promise<Mock> {
  const server = createMockServer({ stepMs, keepaliveMs: 50 });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
  const base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  return {
    base,
    async session(config) {
      const name = `sessions-${++sessions}`;
      // a person of its own: the threads of one test are in no other test's list
      const user = `${name}@example.com`;
      const res = await fetch(
        `${base}/__mock/config?user=${user}&session=${name}${config ? `&${config}` : ""}`,
        { method: "POST" },
      );
      if (res.status !== 204) throw new Error(`config ${config}: ${res.status}`);
      return { headers: { Cookie: `mock-registry=${name}` }, name, user };
    },
    async close() {
      server.closeAllConnections();
      await new Promise<void>((r) => server.close(() => r()));
    },
  };
}

export type Person = {
  headers: Record<string, string>;
  name: string;
  /** The person the session is: their e-mail address. */
  user: string;
};

/** The calls one person makes against one mock. */
export function calls(mock: Mock, who: Person) {
  const json = (method: string, path: string, body?: unknown, extra: Record<string, string> = {}) =>
    fetch(mock.base + path, {
      method,
      headers: { "Content-Type": "application/json", ...who.headers, ...extra },
      ...(body === undefined
        ? {}
        : { body: typeof body === "string" ? body : JSON.stringify(body) }),
    });
  const config = async (query: string) => {
    const res = await fetch(`${mock.base}/__mock/config?${query}&session=${who.name}`, {
      method: "POST",
    });
    if (res.status !== 204) throw new Error(`config ${query}: ${res.status}`);
  };
  const thread = async (id: string): Promise<Thread> =>
    (await (await json("GET", `/api/threads/${id}`)).json()) as Thread;
  const log = async (id: string): Promise<LogEvent[]> => {
    const res = await json("GET", `/api/threads/${id}/export`);
    return ((await res.json()) as { events: LogEvent[] }).events;
  };
  const kinds = async (id: string) => (await log(id)).map((e) => e.kind);
  /** Waits for `test` to hold, polling the mock. */
  const until = async (test: () => Promise<boolean>, what: string) => {
    for (let i = 0; i < 400; i++) {
      if (await test()) return;
      await new Promise((r) => setTimeout(r, 10));
    }
    throw new Error(`timed out waiting for ${what}`);
  };
  const reaches = (id: string, states: Thread["state"][]) =>
    until(async () => states.includes((await thread(id)).state), `${id} in ${states.join("|")}`);
  /** A thread the agent answered (so that it has the context a start needs), in `adam`. */
  const answered = async (text = "echo hello"): Promise<string> => {
    const id = newId();
    const res = await run(mock, who, "adam", id, text);
    if (res.status !== 200) throw new Error(`run: ${res.status}`);
    await res.text();
    await reaches(id, ["done"]);
    return id;
  };
  const start = (parent: string, body: Record<string, unknown> = {}): Promise<Response> =>
    json("POST", `/api/threads/${parent}/sessions`, {
      id: newRequest(),
      agent: "planner",
      description: "Plan the work",
      text: "Plan it",
      ...body,
    });
  const close = (child: string, body?: unknown) =>
    json("POST", `/api/threads/${child}/session/close`, body);
  /** The events of `kind` in the log of `id`. */
  const of = async (id: string, ...wanted: string[]) =>
    (await log(id)).filter((e) => wanted.includes(e.kind));
  /** Starts a session and waits for it to open: the child's id and the session's. */
  const opened = async (
    parent: string,
    body: Record<string, unknown> = {},
  ): Promise<{ child: string; session: string; request: string }> => {
    const request = newRequest();
    const res = await start(parent, { id: request, ...body });
    if (res.status !== 202) throw new Error(`start: ${res.status} ${await res.text()}`);
    await until(
      async () =>
        (await of(parent, "child_session_start_ended")).some((e) => e.data.request === request),
      "the start's outcome",
    );
    const ended = (await of(parent, "child_session_start_ended")).find(
      (e) => e.data.request === request,
    );
    return {
      child: String(ended?.data.thread),
      session: String(ended?.data.session),
      request,
    };
  };
  /** Lets the round (or the starts) of a thread that waits go on; polls until it does, as the round may not have reached its pause yet. */
  const release = async (id: string) => {
    for (let i = 0; i < 300; i++) {
      const res = await fetch(`${mock.base}/__mock/release?thread=${id}`, { method: "POST" });
      if (res.status === 204) return;
      await new Promise((r) => setTimeout(r, 10));
    }
    throw new Error(`nothing of ${id} waited to be released`);
  };
  return {
    json,
    config,
    release,
    thread,
    log,
    kinds,
    until,
    reaches,
    answered,
    start,
    close,
    of,
    opened,
  };
}

/** A run on `agent` with one message, as a person's composer sends it. */
export function run(
  mock: Mock,
  who: Person,
  agent: string,
  threadId: string,
  text: string,
  forwardedProps: Record<string, unknown> = {},
  runId = `run-${++ids}`,
) {
  return fetch(`${mock.base}/agui/agents/${agent}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream", ...who.headers },
    body: JSON.stringify({
      state: {},
      tools: [],
      context: [],
      forwardedProps,
      threadId,
      runId,
      messages: [{ id: `m-${++ids}`, role: "user", content: text }],
    }),
  });
}
