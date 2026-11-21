/*
 * The DOM tests of the subagent dialogs run against the mock orchestrator (`mock/sessions.ts`), which
 * answers as the orchestrator does. This points the page's `fetch` at it, as one person (their own
 * cookie, so that the switches of a test reach no other test), and lets a test answer a request itself
 * with a problem body for what the mock cannot be told to do. Only for tests: nothing imports it.
 */
import { calls, type Mock, type Person, startMock } from "../../../mock/sessions-client";

export type Faked = { status: number; body: Record<string, unknown> };

export type MockPage = {
  mock: Mock;
  who: Person;
  api: ReturnType<typeof calls>;
  /** Answers `METHOD /path` (with ids as `:id`) with a problem; `null` goes back to the mock. */
  fake: (key: string, answer: Faked | null) => void;
  /** Holds the answer to `METHOD /path` until the returned function is called. */
  gate: (key: string) => () => void;
  /** Every request the page made: `METHOD /path status`, ids as `:id`. */
  seen: string[];
  restore: () => Promise<void>;
};

export async function mockPage(): Promise<MockPage> {
  const mock = await startMock();
  const who = await mock.session();
  const realFetch = globalThis.fetch;
  const RealRequest = globalThis.Request;
  const faked = new Map<string, Faked>();
  const seen: string[] = [];
  const gates = new Map<string, Promise<void>>();
  const abs = (input: unknown) =>
    typeof input === "string" && input.startsWith("/") ? mock.base + input : input;
  globalThis.Request = class extends RealRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(abs(input) as RequestInfo, init);
    }
  } as typeof Request;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : undefined;
    const url = new URL(request ? request.url : String(abs(String(input))));
    const method = request?.method ?? init?.method ?? "GET";
    const key = `${method} ${url.pathname.replace(/[0-9a-f]{8}-[0-9a-f-]{27}/g, ":id")}`;
    const answer = faked.get(key);
    if (answer) {
      seen.push(`${key} ${answer.status}`);
      return new Response(JSON.stringify(answer.body), {
        status: answer.status,
        headers: { "content-type": "application/problem+json" },
      });
    }
    await gates.get(key);
    const carried = new RealRequest(abs(input) as RequestInfo, init);
    carried.headers.set("cookie", who.headers.Cookie ?? "");
    const res = await realFetch(carried);
    seen.push(`${key} ${res.status}`);
    return res;
  }) as typeof fetch;
  return {
    mock,
    who,
    api: calls(mock, who),
    fake: (key, answer) => void (answer ? faked.set(key, answer) : faked.delete(key)),
    gate: (key) => {
      let open = () => {};
      gates.set(
        key,
        new Promise<void>((r) => {
          open = r;
        }),
      );
      return () => {
        open();
        gates.delete(key);
      };
    },
    seen,
    restore: async () => {
      globalThis.fetch = realFetch;
      globalThis.Request = RealRequest;
      await mock.close();
    },
  };
}
