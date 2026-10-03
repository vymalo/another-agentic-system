import { describe, expect, it } from "vitest";
import { fakeFetch, LiveStream, loadGolden, problem, sse, THREAD_ID } from "./testing";
import { ThreadAgent } from "./thread-agent";

/*
 * A reader follows a thread by its link's token (ADR 0040, docs/api/agui.md "Reading a shared
 * thread"): the same stream over another route, and the files named by the owner's route are read
 * by the shared one.
 */

const BASE = "http://orch.test";
const TOKEN = "T".repeat(43);

const until = async (test: () => boolean, what = "condition") => {
  for (let i = 0; i < 400; i++) {
    if (test()) return;
    await new Promise((r) => setTimeout(r, 5));
  }
  throw new Error(`timed out waiting for ${what}`);
};

function reader(audience: "internal" | "public", stream = new LiveStream()) {
  const { fetch, calls } = fakeFetch(() => sse(stream.body));
  const agent = new ThreadAgent({
    threadId: THREAD_ID,
    fetch,
    baseUrl: BASE,
    target: () => ({ agentId: "plain", release: null }),
    backoff: () => 1,
    source: { token: TOKEN, audience },
  });
  return { agent, calls, stream };
}

describe("ThreadAgent: a shared thread", () => {
  it("follows the signed-in route for a signed-in reader, from a cursor on a reconnect", async () => {
    const first = new LiveStream();
    const second = new LiveStream();
    const streams = [first, second];
    const { fetch, calls } = fakeFetch(() => sse((streams.shift() as LiveStream).body));
    const agent = new ThreadAgent({
      threadId: THREAD_ID,
      fetch,
      baseUrl: BASE,
      target: () => ({ agentId: "plain", release: null }),
      backoff: () => 1,
      source: { token: TOKEN, audience: "internal" },
    });
    agent.start();
    const echo = loadGolden("connect-echo");
    const at = echo.findIndex((f) => f.id !== undefined);
    first.frames(echo.slice(0, at + 1));
    await until(() => agent.getSnapshot().lastSeq > 0, "the first group");
    const seen = agent.getSnapshot().lastSeq;
    first.cut();
    await until(() => calls.length === 2, "the reconnect");
    expect(calls.map((c) => c.path)).toEqual([
      `/agui/shared/${TOKEN}/connect`,
      `/agui/shared/${TOKEN}/connect`,
    ]);
    expect(calls[1]?.lastEventId).toBe(String(seen));
    agent.stop();
  });

  it("follows the public route for a reader who is not signed in", async () => {
    const { agent, calls } = reader("public");
    agent.start();
    await until(() => calls.length === 1, "the connect");
    expect(calls[0]?.path).toBe(`/agui/public/shared/${TOKEN}/connect`);
    agent.stop();
  });

  it("says the link does not work on the one 404, and stops there", async () => {
    const { fetch, calls } = fakeFetch(() => problem(404, "Not found"));
    const agent = new ThreadAgent({
      threadId: THREAD_ID,
      fetch,
      baseUrl: BASE,
      target: () => ({ agentId: "plain", release: null }),
      backoff: () => 1,
      source: { token: TOKEN, audience: "internal" },
    });
    agent.start();
    await until(() => agent.getSnapshot().notFound, "the 404");
    await new Promise((r) => setTimeout(r, 30));
    expect(calls).toHaveLength(1);
  });

  it("reads a file by the route of the link, not the owner's: the href of the artifact is rewritten", async () => {
    for (const [audience, route] of [
      ["internal", `/api/shared/${TOKEN}/artifacts/`],
      ["public", `/api/public/shared/${TOKEN}/artifacts/`],
    ] as const) {
      const { agent, stream } = reader(audience);
      agent.start();
      stream.frames(loadGolden("file"));
      const seen: string[] = [];
      await until(() => agent.getSnapshot().lastSeq > 0, "the file's run");
      // the run is an external run: its frames carry the artifact activity
      const run = await agent.nextExternalRun();
      await new Promise<void>((resolve) => {
        run?.frames.subscribe({
          next: (e) => {
            const content = (e as { content?: { href?: unknown } }).content;
            if (typeof content?.href === "string") seen.push(content.href);
          },
          complete: resolve,
        });
      });
      expect(seen.length).toBeGreaterThan(0);
      for (const href of seen) {
        expect(href.startsWith(route)).toBe(true);
        expect(href).toMatch(/\/[0-9a-f]{64}$/);
      }
      agent.stop();
    }
  });
});
