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
type CheckSource = components["schemas"]["CheckSource"];

type Ev = Record<string, unknown> & { type: string };
export type Frame = { id?: number; event: Ev };

/** The verification gate a thread's job runs under (ADR 0018): `require` is in the order of the sources. */
export type GateInfo = {
  require: CheckSource[];
  maxAttempts: number;
  /** The agent that verifies, when `verifier` is required (`meta.gate.verifier` of the real projection). */
  verifier?: string;
};

export type ThreadInfo = {
  threadId: string;
  title: string;
  target: { agentId: string; release?: string };
  /** Absent: no gate, so no `job` and a run that ends at the agent's `completed`. */
  gate?: GateInfo;
};

/** What `Thread.job` and the `job` of a `STATE_SNAPSHOT` say (docs/api/chat-api.yaml, `ThreadJob`). */
export type Job = components["schemas"]["ThreadJob"];

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

/** How the verifier's subagent ends: with its verdict, or without one (the round ended elsewhere). */
type VerifierClose = { passed: boolean } | "abandoned";

const SURFACE_OPS = ["createSurface", "updateComponents", "updateDataModel", "deleteSurface"];

/** The surface an A2UI message is about, and what it does: null for anything that is not a message. */
export function inspectOperation(
  op: unknown,
): { surfaceId: string; op: string; version: string } | null {
  if (typeof op !== "object" || op === null || Array.isArray(op)) return null;
  const rec = op as Record<string, unknown>;
  const keys = Object.keys(rec).filter((k) => k !== "version");
  const key = keys[0];
  if (typeof rec.version !== "string" || keys.length !== 1 || !key || !SURFACE_OPS.includes(key)) {
    return null;
  }
  const body = rec[key];
  const id =
    typeof body === "object" && body !== null
      ? (body as Record<string, unknown>).surfaceId
      : undefined;
  return typeof id === "string" ? { surfaceId: id, op: key, version: rec.version } : null;
}

/** The surfaces a log has now, with the version each speaks (deleted ones are gone). */
export function surfacesOf(log: readonly Event[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const e of log) {
    if (e.kind !== "ui_surface" || !Array.isArray(e.data.operations)) continue;
    for (const op of e.data.operations) {
      const info = inspectOperation(op);
      if (!info) continue;
      if (info.op === "deleteSurface") out.delete(info.surfaceId);
      else if (!out.has(info.surfaceId)) out.set(info.surfaceId, info.version);
    }
  }
  return out;
}

type Surface = { messageId: string; operations: unknown[] };

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

const COMMIT = /^[0-9a-f]{40}$/;

/** The commit of a `branch` artifact (the core's `recognise_artifact`, reduced to what `job.sha` needs). */
function pushedCommit(name: unknown, text: unknown): string | undefined {
  if (name !== "branch" || typeof text !== "string") return undefined;
  try {
    const value: unknown = JSON.parse(text);
    const commit = isRecord(value) ? str(value.commit)?.toLowerCase() : undefined;
    return commit !== undefined && COMMIT.test(commit) ? commit : undefined;
  } catch {
    return undefined;
  }
}

export class Projector {
  private state: ThreadState | undefined;
  private run: Open | null = null;
  private invocation: Invocation | null = null;
  /** The verifier's invocation, while a verification waits for its verdict (ADR 0018). */
  private verifier: Invocation | null = null;
  /** A suspended invocation continues under the same id when the thread does. */
  private suspended: Invocation | null = null;
  private interrupt: { id: string; reason: string; message?: string; sub: string } | null = null;
  private failure: { message: string; code: string } | null = null;
  private lastWasError = false;
  private openText: { id: string; said: string } | null = null;
  private readonly said = new Set<string>();
  /** The operations received so far per live surface: every snapshot carries the whole surface. */
  private readonly surfaces = new Map<string, Surface>();
  /** The attempt the agent is on, and the commit it pushed in it (`job` of the snapshot). */
  private attempt = 1;
  private sha: string | undefined;
  /** How many verifications have started: the agent's `completed` under a gate starts one. */
  private verification = 0;
  /** A source failed and nothing has answered it yet: an `error` that follows is the gate out of attempts. */
  private checksFailed = false;
  /** The actor of the agent's last event: a rework starts the next attempt's invocation as it. */
  private lastAgent: Event["actor"] | undefined;

  constructor(private readonly info: ThreadInfo) {}

  get runOpen(): boolean {
    return this.run !== null;
  }

  /** Where the job stands, when the thread has a gate. */
  job(): Job | undefined {
    const gate = this.info.gate;
    if (!gate || gate.require.length === 0) return undefined;
    return {
      attempt: this.attempt,
      maxAttempts: gate.maxAttempts,
      gate: [...gate.require],
      ...(this.sha !== undefined ? { sha: this.sha } : {}),
    };
  }

  private snapshot(): Ev {
    const job = this.job();
    return {
      type: "STATE_SNAPSHOT",
      snapshot: {
        ...(job ? { job } : {}),
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
    if (this.verifier) out.push(this.startedEvent(this.verifier));
    out.push(this.snapshot());
    return out.map((event) => ({ event }));
  }

  /**
   * The verifier starts: a subagent named after the verifier agent, with an id derived from the
   * verification (`sub-verify-<n>`), open from its `pending` card until its verdict.
   */
  private openVerifier(e: Event, out: Ev[]) {
    if (this.verifier) return;
    const name = this.info.gate?.verifier ?? "verifier";
    const inv: Invocation = {
      id: `sub-verify-${Math.max(1, this.verification)}`,
      event: { ...e, actor: { type: "agent", name } },
    };
    this.verifier = inv;
    out.push(this.startedEvent(inv));
  }

  /** The verifier's invocation ends, when one is open. */
  private closeVerifier(how: VerifierClose, out: Ev[]) {
    const inv = this.verifier;
    if (!inv) return;
    this.verifier = null;
    out.push({
      type: "SUBAGENT_FINISHED",
      subagentRunId: inv.id,
      result: how === "abandoned" ? { status: "canceled" } : { passed: how.passed },
    });
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
    if (e.actor.type === "agent") this.lastAgent = e.actor;
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
        // a message during a verification abandons it
        this.closeVerifier("abandoned", out);
        if (this.state === "verifying") this.state = "queued";
        this.checksFailed = false;
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
          // under a gate the agent finishing is not the end: the work is verified, the run stays open
          if (this.job()) {
            this.verification += 1;
            this.state = "verifying";
            out.push(this.snapshot());
          }
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
        // what is verified is what the agent had pushed when it finished
        const pushed = pushedCommit(name, text);
        if (this.job() && this.state !== "verifying" && pushed !== undefined) this.sha = pushed;
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
      case "ui_surface": {
        const inv = this.ensureInvocation(e, out);
        const touched: string[] = [];
        const snapshot = (s: Surface): Ev => ({
          type: "ACTIVITY_SNAPSHOT",
          messageId: s.messageId,
          activityType: "a2ui-surface",
          content: { a2ui_operations: [...s.operations] },
          replace: true,
          subagentRunId: inv.id,
          metadata: actorMeta(e),
        });
        for (const op of Array.isArray(e.data.operations) ? e.data.operations : []) {
          const info = inspectOperation(op);
          if (!info) continue;
          const surface = this.surfaces.get(info.surfaceId) ?? {
            messageId: `a2ui-${e.seq}`,
            operations: [],
          };
          this.surfaces.set(info.surfaceId, surface);
          surface.operations.push(op);
          if (!touched.includes(info.surfaceId)) touched.push(info.surfaceId);
          if (info.op === "deleteSurface") {
            touched.splice(touched.indexOf(info.surfaceId), 1);
            this.surfaces.delete(info.surfaceId);
            out.push(snapshot(surface));
          }
        }
        for (const id of touched) {
          const surface = this.surfaces.get(id);
          if (surface) out.push(snapshot(surface));
        }
        break;
      }
      case "ui_action": {
        // like a user message it answers a blocked thread; unlike one it says nothing in the
        // transcript but a `vymalo.action` activity, and no invocation is open for it
        this.closeVerifier("abandoned", out);
        if (this.state === "blocked" || this.state === "verifying") this.state = "queued";
        this.interrupt = null;
        this.failure = null;
        const { surfaceId, name, sourceComponentId, context } = e.data;
        out.push(
          this.activity(e, "vymalo.action", { surfaceId, name, sourceComponentId, context }),
        );
        break;
      }
      case "check_result": {
        // one card per source in one verification of one attempt, replaced by its later answers; a stale answer is a
        // card of its own and changes nothing else
        const stale = e.data.stale === true;
        const attempt = Number(e.data.attempt) || 1;
        let messageId = `evt-${e.seq}`;
        if (!stale) {
          this.attempt = Math.max(1, attempt);
          // `job.sha` is what the agent pushed (its `branch` artifact), not the commit a check ran on
          if (e.data.status === "failed") this.checksFailed = true;
          if (this.state !== "verifying" && this.state !== "done" && this.state !== "failed") {
            this.state = "verifying";
            out.push(this.snapshot());
          }
          messageId = `check-${attempt}-${Math.max(1, this.verification)}-${String(e.data.source)}`;
        }
        // the verifier is a subagent of its own for as long as its answer is awaited: it starts
        // with its pending card and ends with its verdict
        const verifier = !stale && e.data.source === "verifier";
        if (verifier && e.data.status === "pending") this.openVerifier(e, out);
        out.push({
          type: "ACTIVITY_SNAPSHOT",
          messageId,
          activityType: "vymalo.check",
          content: { ...e.data },
          replace: true,
          metadata: actorMeta(e),
        });
        if (verifier && e.data.status === "passed") this.closeVerifier({ passed: true }, out);
        if (verifier && e.data.status === "failed") this.closeVerifier({ passed: false }, out);
        break;
      }
      case "ci_result": {
        // a CI system reported a check on a commit: a card of its own for every report, never
        // replacing another (the id ends in the report's place in the log); the `check_result` that
        // follows, when the report counts, is what changes the state
        const d = e.data;
        const sha = String(d.sha ?? "");
        const link = typeof d.url === "string" && /^https?:\/\//i.test(d.url) ? d.url : undefined;
        const content: Record<string, unknown> = {
          name: d.name,
          conclusion: d.conclusion,
          passed: ["success", "neutral", "skipped"].includes(String(d.conclusion)),
          sha,
          shortSha: sha.slice(0, 7),
          provider: d.provider,
          repository: d.repository,
        };
        if (typeof d.branch === "string") content.branch = d.branch;
        if (link) content.url = link;
        if (typeof d.summary === "string") content.summary = d.summary;
        out.push({
          type: "ACTIVITY_SNAPSHOT",
          messageId: `ci-${String(d.provider)}-${sha}-${String(d.name)}-${e.seq}`,
          activityType: "vymalo.ci",
          content,
          replace: false,
          metadata: actorMeta(e),
        });
        break;
      }
      case "rework": {
        // the gate failed and the agent is sent back: the divider, then the next attempt's
        // invocation (the delegation is already on its way, so the run shows it working)
        this.closeVerifier("abandoned", out);
        this.attempt = Number(e.data.attempt) || this.attempt + 1;
        this.sha = undefined;
        this.checksFailed = false;
        this.state = "queued";
        out.push({
          type: "ACTIVITY_SNAPSHOT",
          messageId: `rework-${this.attempt}`,
          activityType: "vymalo.rework",
          content: { ...e.data },
          replace: true,
          metadata: actorMeta(e),
        });
        if (!this.invocation) {
          const actor = this.lastAgent ?? {
            type: "agent" as const,
            name: this.info.target.agentId,
          };
          this.invocation = { id: `sub-${e.seq}`, event: { ...e, actor } };
          out.push(this.startedEvent(this.invocation));
        }
        out.push(this.snapshot());
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
        // an error right after a failed check is the gate out of attempts; any other is a delivery
        const code =
          this.checksFailed && e.data.retryable !== true ? "checks_failed" : "delivery_failed";
        this.failure = { message, code };
        this.lastWasError = true;
        break;
      }
      case "thread_state": {
        if (!this.run) this.openRun(e, out);
        this.state = e.data.state as ThreadState;
        // a verifier still waited for when the run ends never answered (a timeout, a cancel, or
        // another source decided the round); the run's success is its pass
        this.closeVerifier(this.state === "done" ? { passed: true } : "abandoned", out);
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
                      : f.code === "checks_failed"
                        ? "Checks failed"
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
