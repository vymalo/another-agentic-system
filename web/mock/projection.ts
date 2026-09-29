/**
 * The mock's copy of the orchestrator's projection of the event log to AG-UI
 * (docs/api/agui.md, `orch-agui-projection`): the same frames, ids and resume points, for the
 * event kinds the mock's scripts emit. `golden.test.ts` replays every scenario of
 * docs/api/examples through the mock and requires the frames of `agui/<name>.agui.json`, so this
 * cannot drift from the real one unnoticed.
 */
import type { components } from "../src/lib/api/schema";

type Event = components["schemas"]["Event"];
type ThreadState = components["schemas"]["ThreadState"];

type Ev = Record<string, unknown> & { type: string };
export type Frame = { id?: number; event: Ev };

export type ThreadInfo = {
  threadId: string;
  title: string;
  target: { agentId: string; release?: string };
};

/** The requester of a run already holds the user messages its own request carried. */
export type Audience = { skipUserMessageIds?: ReadonlySet<string> };

const actorMeta = (e: Event) => ({
  "vymalo.actor": {
    type: e.actor.type,
    name: e.actor.name,
    ...(e.actor.revision ? { revision: e.actor.revision } : {}),
  },
});

const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

type Invocation = { id: string; event: Event };
type Open = { runId: string };

export class Projector {
  private state: ThreadState | undefined;
  private run: Open | null = null;
  private invocation: Invocation | null = null;
  /** A suspended invocation continues under the same id when the thread does. */
  private suspended: Invocation | null = null;
  private interrupt: { id: string; reason: string; message?: string; sub: string } | null = null;
  private failure: { message: string; code: string } | null = null;
  private lastWasError = false;
  private openText: { id: string; said: string } | null = null;
  private readonly said = new Set<string>();

  constructor(private readonly info: ThreadInfo) {}

  get runOpen(): boolean {
    return this.run !== null;
  }

  private snapshot(): Ev {
    return {
      type: "STATE_SNAPSHOT",
      snapshot: {
        thread: {
          state: this.state,
          title: this.info.title,
          target: {
            agentId: this.info.target.agentId,
            ...(this.info.target.release ? { release: this.info.target.release } : {}),
          },
        },
      },
    };
  }

  /** What a cursor inside an open run starts with: the run, its invocation and the thread state. */
  preamble(): Frame[] {
    if (!this.run) return [];
    const out: Ev[] = [
      {
        type: "RUN_STARTED",
        threadId: this.info.threadId,
        runId: this.run.runId,
        protocolVersion: "1.0",
      },
    ];
    if (this.invocation) out.push(this.startedEvent(this.invocation));
    out.push(this.snapshot());
    return out.map((event) => ({ event }));
  }

  private startedEvent(inv: Invocation): Ev {
    return {
      type: "SUBAGENT_STARTED",
      subagentRunId: inv.id,
      name: inv.event.actor.name,
      metadata: actorMeta(inv.event),
    };
  }

  private openRun(e: Event, out: Ev[]) {
    const runId = str(e.data.runId) ?? `run-${e.seq}`;
    this.run = { runId };
    this.interrupt = null;
    this.failure = null;
    this.state = e.kind === "agent_status" && e.data.status === "working" ? "working" : "queued";
    out.push({
      type: "RUN_STARTED",
      threadId: this.info.threadId,
      runId,
      protocolVersion: "1.0",
    });
    out.push(this.snapshot());
  }

  private ensureInvocation(e: Event, out: Ev[]): Invocation {
    if (this.invocation) return this.invocation;
    const inv: Invocation = {
      id: this.suspended ? this.suspended.id : `sub-${e.seq}`,
      event: e,
    };
    this.suspended = null;
    this.invocation = inv;
    out.push(this.startedEvent(inv));
    return inv;
  }

  private activity(
    e: Event,
    activityType: string,
    content: Record<string, unknown>,
    sub?: string,
  ): Ev {
    return {
      type: "ACTIVITY_SNAPSHOT",
      messageId: `evt-${e.seq}`,
      activityType,
      content,
      ...(sub ? { subagentRunId: sub } : {}),
      metadata: actorMeta(e),
    };
  }

  private textTriad(
    e: Event,
    messageId: string,
    text: string,
    role: "user" | "assistant",
    sub?: string,
  ): Ev[] {
    const attr = sub ? { subagentRunId: sub } : {};
    return [
      {
        type: "TEXT_MESSAGE_START",
        messageId,
        role,
        ...(role === "assistant" ? { name: e.actor.name } : {}),
        ...attr,
        metadata: actorMeta(e),
      },
      { type: "TEXT_MESSAGE_CONTENT", messageId, delta: text, ...attr },
      { type: "TEXT_MESSAGE_END", messageId, ...attr },
    ];
  }

  /** Frames of one log event. `id` (the seq) goes on the last frame, unless a message is open. */
  apply(e: Event, audience: Audience = {}): Frame[] {
    const out: Ev[] = [];
    if (!this.run && e.kind !== "user_message" && e.kind !== "thread_state") this.openRun(e, out);
    switch (e.kind) {
      case "user_message": {
        if (!this.run) this.openRun(e, out);
        this.lastWasError = false;
        const messageId = str(e.data.messageId) ?? `evt-${e.seq}`;
        if (!audience.skipUserMessageIds?.has(messageId)) {
          out.push(...this.textTriad(e, messageId, str(e.data.text) ?? "", "user"));
        }
        break;
      }
      case "agent_message": {
        const inv = this.ensureInvocation(e, out);
        const id = str(e.data.messageId) ?? `evt-${e.seq}`;
        const text = str(e.data.text) ?? "";
        const final = e.data.final === true;
        const attr = { subagentRunId: inv.id };
        if (this.openText && text.startsWith(this.openText.said) && this.openText.id === id) {
          const suffix = text.slice(this.openText.said.length);
          if (suffix)
            out.push({ type: "TEXT_MESSAGE_CONTENT", messageId: id, delta: suffix, ...attr });
          this.openText.said = text;
        } else if (this.openText) {
          out.push({ type: "TEXT_MESSAGE_END", messageId: this.openText.id, ...attr });
          this.openText = null;
        }
        if (!this.openText && !(final && this.said.has(`${id}\0${text}`))) {
          if (final) {
            out.push(...this.textTriad(e, id, text, "assistant", inv.id));
            this.said.add(`${id}\0${text}`);
          } else {
            const [start, content] = this.textTriad(e, id, text, "assistant", inv.id);
            out.push(start as Ev, content as Ev);
            this.openText = { id, said: text };
          }
        } else if (final && this.openText) {
          out.push({ type: "TEXT_MESSAGE_END", messageId: this.openText.id, ...attr });
          this.said.add(`${this.openText.id}\0${text}`);
          this.openText = null;
        }
        break;
      }
      case "agent_status": {
        const inv = this.ensureInvocation(e, out);
        const status = str(e.data.status) ?? "working";
        const detail = str(e.data.detail);
        out.push(
          this.activity(
            e,
            "vymalo.status",
            { status, ...(detail !== undefined ? { detail } : {}) },
            inv.id,
          ),
        );
        if (status === "working" && this.state === "queued") {
          this.state = "working";
          out.push(this.snapshot());
        } else if (status === "input_required" || status === "auth_required") {
          const id = `int-${e.seq}`;
          this.interrupt = {
            id,
            reason: status,
            ...(detail !== undefined ? { message: detail } : {}),
            sub: inv.id,
          };
          out.push({
            type: "SUBAGENT_FINISHED",
            subagentRunId: inv.id,
            outcome: { type: "suspended", interruptIds: [id] },
          });
          this.suspended = inv;
          this.invocation = null;
        } else if (status === "completed") {
          out.push({ type: "SUBAGENT_FINISHED", subagentRunId: inv.id });
          this.invocation = null;
        } else if (status === "failed") {
          const message = detail ?? "the agent failed";
          out.push({
            type: "SUBAGENT_ERROR",
            subagentRunId: inv.id,
            message,
            code: "agent_failed",
          });
          this.failure = { message, code: "agent_failed" };
          this.invocation = null;
        } else if (status === "canceled") {
          out.push({
            type: "SUBAGENT_FINISHED",
            subagentRunId: inv.id,
            result: { status: "canceled" },
          });
          this.invocation = null;
        }
        break;
      }
      case "artifact": {
        const inv = this.ensureInvocation(e, out);
        const { name, mimeType, uri, text } = e.data;
        out.push(
          this.activity(
            e,
            "vymalo.artifact",
            {
              name,
              ...(mimeType !== undefined ? { mimeType } : {}),
              ...(uri !== undefined ? { uri } : {}),
              ...(text !== undefined ? { text } : {}),
            },
            inv.id,
          ),
        );
        break;
      }
      case "error": {
        const message = str(e.data.message) ?? "";
        out.push(
          this.activity(
            e,
            "vymalo.error",
            { message, retryable: e.data.retryable === true },
            this.invocation?.id,
          ),
        );
        if (this.invocation) {
          out.push({
            type: "SUBAGENT_ERROR",
            subagentRunId: this.invocation.id,
            message,
            code: "delivery_failed",
          });
          this.invocation = null;
        }
        this.failure = { message, code: "delivery_failed" };
        this.lastWasError = true;
        break;
      }
      case "thread_state": {
        if (!this.run) this.openRun(e, out);
        this.state = e.data.state as ThreadState;
        out.push(this.snapshot());
        const threadId = this.info.threadId;
        const runId = this.run?.runId ?? "";
        if (this.state === "blocked" && this.interrupt && !this.lastWasError) {
          const i = this.interrupt;
          out.push({
            type: "RUN_FINISHED",
            threadId,
            runId,
            outcome: {
              type: "interrupt",
              interrupts: [
                {
                  id: i.id,
                  reason: i.reason,
                  ...(i.message !== undefined ? { message: i.message } : {}),
                  subagentRunId: i.sub,
                  responseSchema: {
                    type: "object",
                    required: ["text"],
                    properties: { text: { type: "string" } },
                  },
                },
              ],
            },
          });
        } else if (this.state === "blocked" || this.state === "failed") {
          const f = this.failure ?? { message: "the agent failed", code: "agent_failed" };
          out.push({
            type: "RUN_ERROR",
            message: f.message,
            code: f.code,
            metadata: {
              "vymalo.problem": {
                type: "about:blank",
                title:
                  f.code === "agent_failed"
                    ? "Agent failed"
                    : f.code === "delivery_failed"
                      ? "Delivery failed"
                      : "Error",
                detail: f.message,
              },
            },
          });
        } else if (this.state === "cancelled") {
          out.push({ type: "RUN_FINISHED", threadId, runId, outcome: { type: "cancelled" } });
        } else {
          out.push({ type: "RUN_FINISHED", threadId, runId, outcome: { type: "success" } });
        }
        this.run = null;
        this.invocation = null;
        this.interrupt = this.state === "blocked" ? this.interrupt : null;
        this.failure = null;
        break;
      }
    }
    return out.map((event, i) => {
      const last = i === out.length - 1;
      return last && !this.openText ? { id: e.seq, event } : { event };
    });
  }
}
