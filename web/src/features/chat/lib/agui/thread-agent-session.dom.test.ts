// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { navigation } from "@/lib/api/session";
import { renewSession, resetSessionState, sessionStatus } from "@/lib/api/session-refresh";
import { fakeFetch, LiveStream, problem, sse, THREAD_ID } from "./testing";
import { ThreadAgent } from "./thread-agent";

/*
 * The connect stream is the first request to meet an expired session (it reconnects by itself): a
 * 401 on it is refreshed and the stream opened again, or, when there is no session to refresh, held
 * while the person signs in (the page is never left), and is the same failure as before when the
 * deployment has no sign-in.
 */

let go: ReturnType<typeof vi.spyOn>;
let stream: LiveStream;
beforeEach(() => {
  resetSessionState();
  window.history.pushState({}, "", `/threads/${THREAD_ID}`);
  go = vi.spyOn(navigation, "go").mockImplementation(() => {});
  stream = new LiveStream();
});
afterEach(() => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** The edge, asked by the browser (`/oauth2/userinfo`): a session or none, as `has` says at each question. */
function edge(has: () => boolean) {
  const asked: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      asked.push(String(input));
      return has() ? new Response("{}", { status: 200 }) : new Response("no", { status: 401 });
    }),
  );
  return asked;
}

/** An orchestrator whose first `n` connects are 401 and whose next one is an open stream. */
const agentWith = (
  unauthorized: number,
  source?: { token: string; audience: "internal" | "public" },
) => {
  let connects = 0;
  const { fetch, calls } = fakeFetch(() =>
    ++connects <= unauthorized
      ? problem(401, "Unauthorized", "the session has expired")
      : sse(stream.body),
  );
  const agent = new ThreadAgent({
    threadId: THREAD_ID,
    fetch,
    baseUrl: "http://orch.test",
    target: () => ({ agentId: "plain", release: null }),
    backoff: () => 1_000_000,
    ...(source ? { source } : {}),
  });
  return { agent, calls };
};
const until = async (test: () => boolean) => {
  await vi.waitFor(() => expect(test()).toBe(true), { timeout: 2_000 });
};

describe("ThreadAgent and an expired session, with a sign-in built in", () => {
  beforeEach(() => vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start"));

  it("a 401 on the connect stream is refreshed and the stream opens again, on the same page", async () => {
    const asked = edge(() => true);
    const { agent, calls } = agentWith(1);
    agent.start();
    await until(() => agent.getSnapshot().connection === "open");
    expect(asked).toEqual(["/oauth2/userinfo"]);
    expect(calls.map((c) => c.path)).toEqual([
      `/agui/threads/${THREAD_ID}/connect`,
      `/agui/threads/${THREAD_ID}/connect`,
    ]);
    expect(agent.getSnapshot().error).toBeNull();
    expect(go).not.toHaveBeenCalled();
    agent.stop();
  });

  it("with no session it waits, the page is kept, and the stream resumes when the person is back", async () => {
    let signedIn = false;
    edge(() => signedIn);
    const { agent, calls } = agentWith(1);
    agent.start();
    await until(() => sessionStatus() === "ended");
    // held: one request, no error line, no redirect, and the stream is not open
    expect(calls).toHaveLength(1);
    expect(agent.getSnapshot().error).toBeNull();
    expect(agent.getSnapshot().connection).toBe("connecting");
    expect(go).not.toHaveBeenCalled();

    signedIn = true;
    await renewSession();
    await until(() => agent.getSnapshot().connection === "open");
    expect(calls).toHaveLength(2);
    expect(sessionStatus()).toBe("ok");
    expect(go).not.toHaveBeenCalled();
    agent.stop();
  });

  it("a run's POST refused on a stale session is refreshed and sent again, whole, and accepted once", async () => {
    const asked = edge(() => true);
    const post = new LiveStream();
    let accepted = 0;
    let refused = 0;
    const { fetch, calls } = fakeFetch((call) => {
      if (call.method !== "POST") return sse(stream.body);
      if (refused++ === 0) return problem(401, "Unauthorized", "the session has expired");
      accepted++;
      return sse(post.body);
    });
    const onAccepted = vi.fn();
    const agent = new ThreadAgent({
      threadId: THREAD_ID,
      fetch,
      baseUrl: "http://orch.test",
      target: () => ({ agentId: "coder", release: null }),
      backoff: () => 1_000_000,
      onAccepted,
    });
    agent.start();
    const seen: Array<{ type: string }> = [];
    agent
      .run({
        threadId: THREAD_ID,
        runId: "run-9",
        state: {},
        tools: [],
        context: [],
        forwardedProps: {},
        messages: [{ id: "m-1", role: "user", content: "build it" }],
      })
      .subscribe({ next: (e) => seen.push(e as { type: string }) });
    await until(() => calls.filter((c) => c.method === "POST").length === 2);
    post.write(
      `data: ${JSON.stringify({ type: "RUN_STARTED", threadId: THREAD_ID, runId: "run-9", protocolVersion: "1.0" })}\n\n`,
    );
    await until(() => seen.length === 1);
    // the run was refused once, the edge was asked once, and the run the orchestrator took is one, with its whole body
    const posts = calls.filter((c) => c.method === "POST");
    expect(posts).toHaveLength(2);
    expect(posts[1]?.body).toEqual(posts[0]?.body);
    expect(JSON.stringify(posts[1]?.body)).toContain("build it");
    expect(asked).toEqual(["/oauth2/userinfo"]);
    expect(accepted).toBe(1);
    expect(onAccepted).toHaveBeenCalledTimes(1);
    expect(seen[0]).toMatchObject({ type: "RUN_STARTED", runId: "run-9" });
    expect(go).not.toHaveBeenCalled();
    agent.stop();
  });

  it("a message sent while the agent works, held for a sign-in, is let go when the page stops", async () => {
    edge(() => false);
    const { fetch, calls } = fakeFetch((call) =>
      call.method === "POST" ? problem(401, "Unauthorized") : sse(stream.body),
    );
    const agent = new ThreadAgent({
      threadId: THREAD_ID,
      fetch,
      baseUrl: "http://orch.test",
      target: () => ({ agentId: "coder", release: null }),
      backoff: () => 1_000_000,
    });
    const sent = agent.sendWhileWorking("also this", "steer").then(
      () => "sent",
      (e: unknown) => e,
    );
    await until(() => sessionStatus() === "ended");
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
    // held, not failed; leaving the page lets it go, with the 401 as it would have been
    agent.stop();
    expect(await sent).toBeInstanceOf(Error);
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
  });

  it("a public reader never asks the edge, and a 401 is what it always was", async () => {
    const asked = edge(() => true);
    const { agent } = agentWith(99, { token: "t".repeat(43), audience: "public" });
    agent.start();
    await until(() => agent.getSnapshot().error !== null);
    expect(agent.getSnapshot().error).toBe("the session has expired");
    expect(asked).toEqual([]);
    expect(sessionStatus()).toBe("ok");
    expect(go).not.toHaveBeenCalled();
    agent.stop();
  });
});

describe("ThreadAgent and an expired session, without a sign-in", () => {
  it("is reported as it was, and nothing navigates or asks", async () => {
    const asked = edge(() => true);
    const { agent } = agentWith(99);
    agent.start();
    await until(() => agent.getSnapshot().error !== null);
    expect(agent.getSnapshot().error).toBe("the session has expired");
    expect(asked).toEqual([]);
    expect(go).not.toHaveBeenCalled();
    agent.stop();
  });
});
