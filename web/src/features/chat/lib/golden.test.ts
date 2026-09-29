import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { STATUS_TEXT } from "@/features/chat/components/parts/status-line";
import { type ApiEvent, EVENT_KINDS, type ThreadState } from "@/lib/api/types";
import { applyEvents, emptyLog, lastThreadStateEvent, visibleEvents } from "./event-log";
import { convertMessage, toItems } from "./to-items";

/**
 * The golden transcripts in docs/api/examples are what the real orchestrator emitted for each
 * scripted agent behaviour (written by orchestrator/crates/e2e/tests/golden.rs, which fails
 * when they go stale). This is the consumer side: the chat surface must understand every one
 * of them, not only satisfy the schema.
 */

const DIR = path.resolve(import.meta.dirname, "../../../../../docs/api/examples");
const THREAD_ID = "22222222-2222-4222-8222-222222222222";

/** The scenario and the state its thread ends in. */
const EXPECTED_FINAL: Record<string, ThreadState> = {
  echo: "done",
  ask: "done",
  cancel: "cancelled",
  fail: "failed",
  talk: "done",
  release: "done",
};

function load(name: string): ApiEvent[] {
  const raw = JSON.parse(readFileSync(path.join(DIR, `${name}.events.json`), "utf8")) as ApiEvent[];
  // undo the normalisation of the file: the reducer only accepts events of its own thread
  return raw.map((e) => ({ ...e, threadId: THREAD_ID, at: "2026-09-29T09:00:00Z" }));
}

const logOf = (events: ApiEvent[]) => applyEvents(emptyLog(THREAD_ID), events);

describe("golden transcripts (docs/api/examples)", () => {
  it("the directory holds exactly the scenarios the orchestrator test writes", () => {
    const files = readdirSync(DIR)
      .filter((f) => f.endsWith(".events.json"))
      .map((f) => f.replace(/\.events\.json$/, ""))
      .sort();
    expect(files).toEqual(Object.keys(EXPECTED_FINAL).sort());
  });

  it("every golden transcript maps to items without unknown kinds or statuses", () => {
    for (const [name, finalState] of Object.entries(EXPECTED_FINAL)) {
      const events = load(name);
      const log = logOf(events);

      // nothing is dropped as malformed, and seq is contiguous from 1
      expect(log.ordered, `${name}: events kept`).toHaveLength(events.length);
      expect(
        log.ordered.map((e) => e.seq),
        `${name}: seq`,
      ).toEqual(events.map((_, i) => i + 1));

      for (const e of log.ordered) {
        expect(EVENT_KINDS as readonly string[], `${name}: kind ${e.kind}`).toContain(e.kind);
        if (e.kind === "agent_status") {
          expect(Object.keys(STATUS_TEXT), `${name}: status ${e.data.status}`).toContain(
            e.data.status,
          );
        }
        // the orchestrator reports every agent message as final: never a partial
        if (e.kind === "agent_message") expect(e.data.final, `${name}: final`).toBe(true);
      }

      // one transcript item per event, except thread_state (the header badge shows it)
      const visible = visibleEvents(log);
      const items = toItems(visible);
      expect(items, `${name}: items`).toHaveLength(
        events.filter((e) => e.kind !== "thread_state").length,
      );
      for (const item of items) {
        const message = convertMessage(item, 0);
        expect(message, `${name}: item ${item.id}`).toBeDefined();
      }

      expect(lastThreadStateEvent(log)?.state, `${name}: final state`).toBe(finalState);
    }
  });

  it("the failure transcript yields a failed status line with the agent's detail", () => {
    const items = toItems(visibleEvents(logOf(load("fail"))));
    expect(items.find((i) => i.kind === "status" && i.status === "failed")).toMatchObject({
      detail: "scripted failure",
    });
    // an agent failure is not an error event
    expect(items.some((i) => i.kind === "error")).toBe(false);
  });

  it("the PR artifact yields a link card", () => {
    for (const name of ["echo", "ask", "talk", "release"]) {
      const items = toItems(visibleEvents(logOf(load(name))));
      const artifact = items.find((i) => i.kind === "artifact");
      expect(artifact, `${name}: artifact item`).toBeDefined();
      if (artifact?.kind !== "artifact") continue;
      const part = convertMessage(artifact, 0);
      expect(part.content?.[0], name).toMatchObject({
        type: "data-artifact",
        data: {
          name: "result",
          pr: { label: "acme/demo#1", href: "https://github.com/acme/demo/pull/1" },
        },
      });
    }
  });

  it("a blocked run shows the agent's question, and the answer comes back as a user message", () => {
    const items = toItems(visibleEvents(logOf(load("ask"))));
    expect(items.find((i) => i.kind === "status" && i.status === "input_required")).toMatchObject({
      detail: "Which branch?",
    });
    expect(items.filter((i) => i.kind === "user").map((i) => i.kind === "user" && i.text)).toEqual([
      "ask about branches",
      "main",
    ]);
  });

  it("an agent that talks shows its status text and its message once, as agent text", () => {
    const items = toItems(visibleEvents(logOf(load("talk"))));
    const texts = items.filter((i) => i.kind === "agent_text");
    expect(texts).toHaveLength(1);
    expect(texts[0]).toMatchObject({ text: "Plan: add a test", final: true });
    expect(items.find((i) => i.kind === "status" && i.detail)).toMatchObject({
      status: "working",
      detail: "Reading the repository",
    });
  });

  it("a release run echoes the revision on every agent event", () => {
    const events = load("release").filter((e) => e.actor.type === "agent");
    expect(events.length).toBeGreaterThan(0);
    for (const e of events) expect(e.actor.revision).toBe("coder-r51");
  });
});
