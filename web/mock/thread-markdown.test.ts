import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { markdownFilename, threadToMarkdown } from "../src/features/chat/lib/thread-markdown";
import type { components } from "../src/lib/api/schema";
import { postRun } from "./agui-client";
import { createMockServer } from "./server";
import { GOLDEN_AT, GOLDEN_ID } from "./thread-markdown-golden";

/**
 * The golden files of docs/api/thread-markdown-v1.md: threads the mock plays (a plain chat, code and math, a form with its answer,
 * a fork made by regenerating, a kept file), exported through the mock's own `GET /api/threads/{id}/export`, and the exact Markdown
 * `threadToMarkdown` makes of each. The id and the time of the export are the only things a run changes, so they are fixed before
 * the comparison; `e2e/export.spec.ts` compares the file the browser downloads with the same files, the same way.
 * `UPDATE_GOLDEN=1` rewrites them.
 */

type ThreadExport = components["schemas"]["ThreadExport"];

const DIR = path.resolve(import.meta.dirname, "thread-markdown");

const server = createMockServer({ stepMs: 2, keepaliveMs: 1000 });
let base = "";
beforeAll(async () => {
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(async () => {
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});

const json = async (url: string, init?: RequestInit) => (await fetch(`${base}${url}`, init)).json();
const waitFor = async (id: string, state: string) => {
  for (let i = 0; i < 1000; i++) {
    if ((await json(`/api/threads/${id}`)).state === state) return;
    await new Promise((r) => setTimeout(r, 5));
  }
  throw new Error(`thread ${id} never reached ${state}`);
};
const say = async (id: string, n: number, content: string) => {
  const res = await postRun(base, "reviewer", {
    threadId: id,
    runId: `run-${n}`,
    messages: [{ id: `evt-${n}`, role: "user", content }],
  });
  expect(res.status).toBe(200);
  await res.text();
  await waitFor(id, "done");
};
const exported = (id: string): Promise<ThreadExport> => json(`/api/threads/${id}/export`);
const fixed = (doc: ThreadExport): ThreadExport => ({
  ...doc,
  exportedAt: GOLDEN_AT,
  thread: { ...doc.thread, id: GOLDEN_ID },
});

const SCENARIOS: Record<string, () => Promise<ThreadExport>> = {
  "plain-chat": async () => {
    const id = crypto.randomUUID();
    await say(id, 1, "talk");
    await say(id, 6, "steps go");
    return exported(id);
  },
  "code-math": async () => {
    const id = crypto.randomUUID();
    await say(id, 1, "code-math please");
    return exported(id);
  },
  "form-answer": async () => {
    const id = crypto.randomUUID();
    const first = await postRun(base, "reviewer", {
      threadId: id,
      runId: "run-1",
      messages: [{ id: "evt-1", role: "user", content: "choices now" }],
    });
    await first.text();
    await waitFor(id, "blocked");
    const answer = await postRun(base, "reviewer", {
      threadId: id,
      runId: "run-2",
      messages: [],
      forwardedProps: {
        a2uiAction: {
          userAction: {
            name: "answer",
            surfaceId: "s1",
            sourceComponentId: "pick",
            context: {
              answers: [
                { id: "db", values: ["pg"] },
                { id: "auth", values: ["keycloak"] },
              ],
              note: "Use *our* realm",
            },
          },
        },
      },
    });
    await answer.text();
    await waitFor(id, "done");
    return exported(id);
  },
  "fork-regenerate": async () => {
    const id = crypto.randomUUID();
    await say(id, 1, "echo first");
    await say(id, 6, "echo second");
    const second = (await exported(id)).events.filter((e) => e.kind === "user_message")[1];
    const res = await fetch(`${base}/api/threads/${id}/fork`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ replace: second?.seq, text: "echo second", messageId: "evt-regen" }),
    });
    expect(res.status).toBeLessThan(300);
    const fork = (await res.json()) as { id: string };
    await waitFor(fork.id, "done");
    return exported(fork.id);
  },
  "kept-file": async () => {
    const id = crypto.randomUUID();
    await say(id, 1, "file make a chart");
    return exported(id);
  },
};

const golden = (name: string) => path.join(DIR, `${name}.md`);

describe("Export Markdown: the golden files", () => {
  for (const [name, play] of Object.entries(SCENARIOS)) {
    it(`${name}: the mock's thread is exactly the checked-in file`, async () => {
      const markdown = threadToMarkdown(fixed(await play()));
      if (process.env.UPDATE_GOLDEN) {
        mkdirSync(DIR, { recursive: true });
        writeFileSync(golden(name), markdown);
      }
      expect(markdown).toBe(readFileSync(golden(name), "utf8"));
    });
  }

  it("a fork holds one branch: the parent's turns up to the edit, then its own", async () => {
    const markdown = threadToMarkdown(
      fixed(await (SCENARIOS["fork-regenerate"] as () => Promise<ThreadExport>)()),
    );
    expect(markdown.match(/^## Person$/gm)).toHaveLength(2);
    expect(markdown).toContain("From here this is a new version");
    expect(markdown.match(/echo second/g)?.length).toBe(2);
  });

  it("is named after the thread", () => {
    expect(markdownFilename(GOLDEN_ID)).toBe(`thread-${GOLDEN_ID}.md`);
    expect(markdownFilename("../../x")).toBe("thread-x.md");
  });
});
