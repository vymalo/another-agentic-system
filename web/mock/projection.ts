/**
 * The mock's copy of the orchestrator's projection of the event log to AG-UI
 * (docs/api/agui.md, `orch-agui-projection`): the same frames, ids and resume points, for the
 * event kinds the mock's scripts emit. `golden.test.ts` replays every scenario of
 * docs/api/examples through the mock and requires the frames of `agui/<name>.agui.json`, so this
 * cannot drift from the real one unnoticed.
 */
import type { components } from "../src/lib/api/schema";
import { previewOf } from "./files";

type Event = components["schemas"]["Event"];
type ThreadState = components["schemas"]["ThreadState"];
type CheckSource = components["schemas"]["CheckSource"];
type ArtifactFile = components["schemas"]["ArtifactFile"];

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
  /** The description the thread has (ADR 0035); absent when it has none. Part of every snapshot that has one. */
  description?: string;
  target: { agentId: string; release?: string };
  /** Absent: no gate, so no `job` and a run that ends at the agent's `completed`. */
  gate?: GateInfo;
};

/** What `Thread.job` and the `job` of a `STATE_SNAPSHOT` say (docs/api/chat-api.yaml, `ThreadJob`). */
export type Job = components["schemas"]["ThreadJob"];

/** Which UI catalog (ADR 0023), without its contents: `thread.uiCatalog` of the state snapshot. */
export type CatalogRef = { catalogId: string; version: number; digest: string };

/**
 * What a thread knows of the catalogs its screens sent: the digests recorded and the current
 * one, the highest version (the rule of `UiCatalogLedger::observe` of the real core). A digest the
 * thread knows changes nothing; an unseen one is recorded and becomes current when the thread had
 * none, or its version is at least the current one's (the same version with another digest: the
 * later wins). An older version is recorded, never current.
 */
export class CatalogLedger {
  current: CatalogRef | undefined;
  private readonly seen = new Set<string>();

  knows(digest: string): boolean {
    return this.seen.has(digest);
  }

  observe(ref: CatalogRef): { recorded: boolean; becameCurrent: boolean } {
    if (this.seen.has(ref.digest)) return { recorded: false, becameCurrent: false };
    this.seen.add(ref.digest);
    const becameCurrent = this.current === undefined || ref.version >= this.current.version;
    if (becameCurrent) this.current = ref;
    return { recorded: true, becameCurrent };
  }
}

/** The reference a `ui_catalog` event's data names, when it is well formed. */
export function catalogRefOf(data: unknown): CatalogRef | undefined {
  if (typeof data !== "object" || data === null) return undefined;
  const { catalogId, version, digest } = data as Record<string, unknown>;
  if (typeof catalogId !== "string" || typeof digest !== "string") return undefined;
  if (typeof version !== "number" || !Number.isInteger(version)) return undefined;
  return { catalogId, version, digest };
}

/** The requester of a run already holds the user messages its own request carried. */
export type Audience = { skipUserMessageIds?: ReadonlySet<string> };

export const actorMetaOf = (actor: Event["actor"]) => ({
  "vymalo.actor": {
    type: actor.type,
    name: actor.name,
    ...(actor.revision ? { revision: actor.revision } : {}),
  },
});
const actorMeta = (e: Event) => actorMetaOf(e.actor);

const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

type Invocation = { id: string; event: Event };

/**
 * A step of the agent's work that has not ended (ADR 0025): what the projection says about it, so
 * that every later event of the step says it again as the same activity (the real projection's
 * `StepView`).
 */
type StepView = {
  /** The seq of the step's first event: `step-<seq>` is its activity and `sub-step-<seq>` its subagent. */
  seq: number;
  data: Record<string, unknown>;
  startedAt: string;
  actor: Event["actor"];
  path: string[];
  /** Its subagent, for a sub-agent step, and the subagent it was started in. */
  sub?: string;
  parent?: string;
  /** Its `SUBAGENT_STARTED` is open in the run that is open. */
  subOpen: boolean;
};
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

// ---- the core's `recognise_artifact`, ported (orch-core `gate.rs`) ---------------------------

/** A full commit hash: 40 or 64 lower-case hex digits (`is_commit_hash`). */
const isCommitHash = (s: string) => /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(s);

const utf8Length = (s: string) => new TextEncoder().encode(s).length;

/** Whether git accepts `name` as a branch name (`is_branch_name`). */
export function isBranchName(name: string): boolean {
  if (
    name === "" ||
    utf8Length(name) > 255 ||
    name === "@" ||
    name.startsWith("-") ||
    name.endsWith(".") ||
    name.includes("..") ||
    name.includes("@{") ||
    name.includes("//")
  ) {
    return false;
  }
  // biome-ignore lint/suspicious/noControlCharactersInRegex: git refuses control characters
  if (/[\s\u0000-\u001f\u007f`~^:?*[\\]/.test(name)) return false;
  return name
    .split("/")
    .every((part) => part !== "" && !part.startsWith(".") && !part.endsWith(".lock"));
}

function scpToPath(rest: string): string {
  const slash = rest.indexOf("/");
  const [authority, path] = slash < 0 ? [rest, ""] : [rest.slice(0, slash), rest.slice(slash + 1)];
  const colon = authority.indexOf(":");
  if (colon < 0) return rest;
  const host = authority.slice(0, colon);
  const after = authority.slice(colon + 1);
  if (after === "" || /^\d+$/.test(after)) return rest;
  return `${host}/${path === "" ? after : `${after}/${path}`}`;
}

/** The `host/owner/name` key of a repository address (`repo_key`): lower case, no `.git`. */
export function repoKey(raw: string): string | undefined {
  const lowered = raw.trim().toLowerCase();
  // biome-ignore lint/suspicious/noControlCharactersInRegex: an address has no control characters
  if (/[\s\u0000-\u001f\u007f]/.test(lowered)) return undefined;
  const sep = lowered.indexOf("://");
  const scheme = sep < 0 ? undefined : lowered.slice(0, sep);
  let rest = sep < 0 ? lowered : lowered.slice(sep + 3);
  rest = rest.split(/[?#]/)[0] ?? "";
  if (scheme === undefined) rest = scpToPath(rest);
  const parts = rest.split("/");
  let host = (parts[0] ?? "").split("@").pop() ?? "";
  const port =
    scheme === "https" ? ":443" : scheme === "http" ? ":80" : scheme === "ssh" ? ":22" : "";
  if (port && host.endsWith(port)) host = host.slice(0, -port.length);
  if (host === "") return undefined;
  const segments = parts.slice(1).filter((s) => s !== "");
  if (segments.some((s) => s === "." || s === "..")) return undefined;
  const last = segments.length - 1;
  if (last < 0) return undefined;
  const lastSegment = segments[last] ?? "";
  if (lastSegment.endsWith(".git")) segments[last] = lastSegment.slice(0, -4);
  if (segments.length < 2 || segments.some((s) => s === "")) return undefined;
  return `${host}/${segments.join("/")}`;
}

const parseObject = (text: unknown): Record<string, unknown> | undefined => {
  if (typeof text !== "string") return undefined;
  try {
    const value: unknown = JSON.parse(text);
    return isRecord(value) ? value : undefined;
  } catch {
    return undefined;
  }
};

/** The URL of a pull request artifact, when it is one and its URL is usable (`pull_request_url`). */
export function pullRequestUrl(name: string, uri: unknown, text: unknown): string | undefined {
  if (name.replace(/[_-]/g, " ").toLowerCase().trim() !== "pull request") return undefined;
  const payload = parseObject(text);
  const url =
    typeof uri === "string" ? uri : (str(payload?.url) ?? str(payload?.html_url) ?? undefined);
  if (url === undefined) return undefined;
  const authority = url.startsWith("https://")
    ? url.slice("https://".length).split(/[/?#]/)[0]
    : undefined;
  const plausible =
    utf8Length(url) <= 2048 &&
    authority !== undefined &&
    authority !== "" &&
    !authority.includes("@") &&
    // biome-ignore lint/suspicious/noControlCharactersInRegex: a link has no control characters
    !/[\s\u0000-\u001f\u007f-\u009f\\]/.test(url);
  return plausible ? url : undefined;
}

/** The repository and number a pull request URL names (`pull_request_location`), else nothing. */
function pullRequestLocation(url: string): { repository: string; number: number } | undefined {
  const path = url.split(/[?#]/)[0] ?? "";
  for (const marker of ["/-/merge_requests/", "/merge_requests/", "/pulls/", "/pull/"]) {
    const at = path.lastIndexOf(marker);
    if (at < 0) continue;
    const digits = path.slice(at + marker.length).split("/")[0] ?? "";
    const repository = repoKey(path.slice(0, at));
    if (!/^\d+$/.test(digits) || repository === undefined) return undefined;
    return { repository, number: Number(digits) };
  }
  return undefined;
}

/**
 * What a `vymalo.artifact` adds to the artifact as sent: its `kind` and the fields a card needs
 * (the real projection's `typed_artifact`). A `branch` or `checks` artifact that cannot be used is
 * a `file`.
 */
export function typedArtifact(
  name: unknown,
  uri: unknown,
  text: unknown,
  file?: ArtifactFile,
  mimeType?: unknown,
  threadId?: string,
): Record<string, unknown> {
  // a file the artifact store keeps (ADR 0032): where to fetch it, how big, what a preview is
  if (file !== undefined && threadId !== undefined) {
    return {
      kind: "file",
      href: `/api/threads/${threadId}/artifacts/${file.sha256}`,
      sha256: file.sha256,
      size: file.size,
      ...(file.filename !== undefined ? { filename: file.filename } : {}),
      preview: previewOf(typeof mimeType === "string" ? mimeType : undefined),
    };
  }
  const n = typeof name === "string" ? name : "";
  const url = pullRequestUrl(n, uri, text);
  if (url !== undefined) {
    const payload = parseObject(text);
    // where the link goes is what the URL says: never the payload's repository and number
    const location = pullRequestLocation(url);
    const number = location?.number;
    const repository = location?.repository;
    const branch = str(payload?.branch)?.trim();
    return {
      kind: "pull_request",
      url,
      ...(number !== undefined ? { number } : {}),
      ...(repository !== undefined ? { repository } : {}),
      ...(branch !== undefined && isBranchName(branch) ? { branch } : {}),
    };
  }
  const payload = n === "branch" || n === "checks" ? parseObject(text) : undefined;
  const commit = str(payload?.commit)?.toLowerCase();
  if (payload && commit !== undefined && isCommitHash(commit)) {
    if (n === "branch") {
      const repository =
        typeof payload.repository === "string" ? repoKey(payload.repository) : undefined;
      const branch = str(payload.branch)?.trim();
      if (repository !== undefined && branch && isBranchName(branch)) {
        return { kind: "branch", repository, branch, shortSha: commit.slice(0, 7), sha: commit };
      }
    } else if (
      typeof payload.passed === "boolean" &&
      (payload.findings === undefined ||
        payload.findings === null ||
        Array.isArray(payload.findings))
    ) {
      return { kind: "checks", passed: payload.passed, shortSha: commit.slice(0, 7), sha: commit };
    }
  }
  return { kind: "file" };
}

/** The commit of a `branch` artifact (what `job.sha` needs of `recognise_artifact`). */
function pushedCommit(name: unknown, uri: unknown, text: unknown): string | undefined {
  const typed = typedArtifact(name, uri, text);
  return typed.kind === "branch" ? (typed.sha as string) : undefined;
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
  /** The UI catalogs the log recorded, and which is current (ADR 0023); a fork starts with none. */
  private catalog = new CatalogLedger();
  /** Where the thread was forked from, once its `thread_forked` is read: every snapshot says so (ADR 0029). */
  private forkedFrom: { threadId: string; seq: number; kind: string } | undefined;
  /**
   * The MCP servers attached to the thread now (ADR 0024), by id: folded from `tools_attached` and
   * `tools_detached`. They belong to the conversation, so a new job keeps them; every snapshot
   * says them as `thread.tools` (sorted, no member when there are none).
   */
  private readonly tools = new Set<string>();
  /** Which job of the thread the log is in (from 1; `job_started` moves it, ADR 0020). */
  private jobNumber = 1;
  /** The attempt the agent is on, and the commit it pushed in it (`job` of the snapshot). */
  private attempt = 1;
  private sha: string | undefined;
  /** How many verifications have started: the agent's `completed` under a gate starts one. */
  private verification = 0;
  /** A source failed and nothing has answered it yet: an `error` that follows is the gate out of attempts. */
  private checksFailed = false;
  /** The actor of the agent's last event: a rework starts the next attempt's invocation as it. */
  private lastAgent: Event["actor"] | undefined;
  /** The open invocation's last final agent message: a status with the same words says nothing more. */
  private lastFinal: string | undefined;
  /** The steps that have not ended, by id (ADR 0025). */
  private readonly steps = new Map<string, StepView>();
  /** The time of the event being applied, for the frames that close what is open. */
  private now = "";

  constructor(private info: ThreadInfo) {}

  get runOpen(): boolean {
    return this.run !== null;
  }

  /**
   * The agent's invocation, while one is open: its subagent run id, its name and its actor. What
   * the live words of a reply are attributed to (the real projection's `open_invocation`).
   */
  openInvocation(): { id: string; name: string; actor: Event["actor"] } | undefined {
    return this.invocation
      ? {
          id: this.invocation.id,
          name: this.invocation.event.actor.name,
          actor: this.invocation.event.actor,
        }
      : undefined;
  }

  /** Whether the log has said an agent message with this id: live text for it is late. */
  hasMessage(id: string): boolean {
    if (this.openText?.id === id) return true;
    for (const key of this.said) if (key.startsWith(`${id}\0`)) return true;
    return false;
  }

  /** Where the job stands, when the thread has a gate. */
  job(): Job | undefined {
    const gate = this.info.gate;
    if (!gate || gate.require.length === 0) return undefined;
    return {
      ...(this.jobNumber > 1 ? { number: this.jobNumber } : {}),
      attempt: this.attempt,
      maxAttempts: gate.maxAttempts,
      gate: [...gate.require],
      ...(this.sha !== undefined ? { sha: this.sha } : {}),
    };
  }

  /**
   * The thread's next job starts (ADR 0020): the finished one is forgotten (attempt 1, nothing
   * pushed, no surface to act on). The verification count goes on, as the core's does.
   */
  private beginJob(number: number) {
    this.jobNumber = number;
    this.state = "queued";
    this.forgetJob();
  }

  /** What a finished job leaves behind and the next must not inherit (the real projection's `forget_job`). */
  private forgetJob() {
    this.attempt = 1;
    this.sha = undefined;
    this.checksFailed = false;
    this.interrupt = null;
    this.failure = null;
    this.suspended = null;
    this.surfaces.clear();
    this.steps.clear();
  }

  /**
   * The thread began as a copy of another (the real projection's `on_thread_forked`, ADR 0029): a
   * run the copy left open is closed as cancelled, the finished job and the UI catalog are
   * forgotten, the title is the parent's and every snapshot from here on says where the thread
   * came from; a run of its own holds the marker `vymalo.fork` and ends in success.
   */
  private onThreadForked(e: Event, out: Ev[]) {
    const d = e.data as {
      from: { threadId: string; seq: number };
      kind: string;
      title: string;
      description?: string;
      target: { agentId: string; release?: string };
    };
    this.state = "done";
    // the fork has its parent's title and description as they were when it was made
    this.info = { ...this.info, title: d.title, description: d.description || undefined };
    this.forkedFrom = { threadId: d.from.threadId, seq: d.from.seq, kind: d.kind };
    const threadId = this.info.threadId;
    if (this.run) {
      if (this.openText) {
        out.push({ type: "TEXT_MESSAGE_END", messageId: this.openText.id });
        this.openText = null;
      }
      this.closeVerifier("abandoned", out);
      if (this.invocation) {
        this.closeSteps("canceled", out);
        out.push(Projector.canceledSubagent(this.invocation.id));
      }
      out.push(this.snapshot());
      out.push({
        type: "RUN_FINISHED",
        threadId,
        runId: this.run.runId,
        outcome: { type: "cancelled" },
      });
      this.run = null;
      this.invocation = null;
    }
    this.forgetJob();
    this.catalog = new CatalogLedger();
    this.lastFinal = undefined;
    this.lastWasError = false;
    const runId = `run-${e.seq}`;
    out.push({ type: "RUN_STARTED", threadId, runId, protocolVersion: "1.0" });
    out.push({
      type: "ACTIVITY_SNAPSHOT",
      messageId: `fork-${e.seq}`,
      activityType: "vymalo.fork",
      content: {
        from: { threadId: d.from.threadId, seq: d.from.seq },
        kind: d.kind,
        title: d.title,
        target: d.target,
        at: e.at,
      },
      metadata: actorMeta(e),
    });
    out.push(this.snapshot());
    out.push({ type: "RUN_FINISHED", threadId, runId, outcome: { type: "success" } });
  }

  /**
   * MCP servers were attached to the thread, or detached from it (the real projection's `on_tools`,
   * ADR 0024). Inside a run: the new `STATE_SNAPSHOT`, then the `vymalo.tools` card. Outside any
   * run, with the thread active: the run opens (its own snapshot says the new set), then the card.
   * With the thread finished or waiting: a run of its own, the card, the snapshot and the run's
   * close by the state the thread is in, which the web drops as material-less. Ids only.
   */
  private onTools(e: Event, out: Ev[]) {
    const ids = Array.isArray(e.data.servers)
      ? e.data.servers.filter((s): s is string => typeof s === "string")
      : [];
    const attached = e.kind === "tools_attached";
    for (const id of ids) {
      if (attached) this.tools.add(id);
      else this.tools.delete(id);
    }
    const card = this.activity(e, "vymalo.tools", { [attached ? "attached" : "detached"]: ids });
    const active =
      this.state === "queued" || this.state === "working" || this.state === "verifying";
    const threadId = this.info.threadId;
    if (this.run) {
      out.push(this.snapshot());
      out.push(card);
      return;
    }
    const runId = `run-${e.seq}`;
    out.push({ type: "RUN_STARTED", threadId, runId, protocolVersion: "1.0" });
    this.run = { runId };
    if (active) {
      out.push(this.snapshot());
      out.push(card);
      return;
    }
    out.push(card);
    out.push(this.snapshot());
    out.push(this.runEnd());
    this.run = null;
    this.invocation = null;
  }

  private snapshot(): Ev {
    const job = this.job();
    return {
      type: "STATE_SNAPSHOT",
      snapshot: {
        ...(job ? { job } : {}),
        thread: {
          ...(this.jobNumber > 1 ? { jobNumber: this.jobNumber } : {}),
          ...(this.catalog.current ? { uiCatalog: this.catalog.current } : {}),
          ...(this.forkedFrom ? { forkedFrom: this.forkedFrom } : {}),
          ...(this.tools.size > 0 ? { tools: [...this.tools].sort() } : {}),
          state: this.state,
          title: this.info.title,
          ...(this.info.description ? { description: this.info.description } : {}),
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
    // the step subagents that are open, parents first: what a step says next is attributed to one
    for (const id of this.openStepSubagents(true)) {
      const step = this.steps.get(id);
      if (step) out.push(this.stepStarted(step));
    }
    out.push(this.snapshot());
    return out.map((event) => ({ event }));
  }

  // ---- steps (ADR 0025, the real projection's `on_agent_step`) -------------------------------

  /** The subagent a step with `path` is attributed to: the nearest ancestor whose subagent is open, else the invocation. */
  private enclosingRun(path: readonly string[]): string | undefined {
    for (const id of [...path].reverse()) {
      const step = this.steps.get(id);
      if (step?.subOpen && step.sub) return step.sub;
    }
    return this.invocation?.id;
  }

  /** The `vymalo.step` snapshot of the step `id` as it stands, at the time of the event being applied. */
  private stepActivity(id: string): Ev | undefined {
    const step = this.steps.get(id);
    if (!step) return undefined;
    const subagentRunId = this.enclosingRun(step.path);
    return {
      type: "ACTIVITY_SNAPSHOT",
      messageId: `step-${step.seq}`,
      activityType: "vymalo.step",
      content: { ...step.data, startedAt: step.startedAt, at: this.now },
      replace: true,
      ...(subagentRunId ? { subagentRunId } : {}),
      metadata: actorMetaOf(step.actor),
    };
  }

  /** `SUBAGENT_STARTED` of a step's subagent, in the subagent it was started in. */
  private stepStarted(step: StepView): Ev {
    return {
      type: "SUBAGENT_STARTED",
      subagentRunId: step.sub,
      name: step.data.label,
      ...(step.parent ? { parentSubagentRunId: step.parent } : {}),
      metadata: actorMetaOf(step.actor),
    };
  }

  /** The ids of the steps whose subagent is open: outermost first, or deepest first. */
  private openStepSubagents(parentsFirst: boolean): string[] {
    const open = [...this.steps.values()]
      .filter((s) => s.subOpen)
      .sort((a, b) => a.path.length - b.path.length || a.seq - b.seq);
    if (!parentsFirst) open.reverse();
    return open.map((s) => String(s.data.id));
  }

  /** A subagent that was cut short (the 1.0 outcome union has no cancelled member). */
  private static canceledSubagent(id: string | undefined): Ev {
    return { type: "SUBAGENT_FINISHED", subagentRunId: id, result: { status: "canceled" } };
  }

  /**
   * The invocation is closing: what it still has open ends first, deepest first. When it suspends,
   * the step subagents suspend with it (and are not started again); otherwise every step that has
   * not ended is canceled, as a snapshot (no spinner stays) and, for a sub-agent step, as the end
   * of its subagent.
   */
  private closeSteps(how: "suspended" | "canceled", out: Ev[]) {
    if (how === "suspended") {
      for (const id of this.openStepSubagents(false)) {
        const step = this.steps.get(id);
        if (!step) continue;
        step.subOpen = false;
        out.push({
          type: "SUBAGENT_FINISHED",
          subagentRunId: step.sub,
          outcome: { type: "suspended" },
        });
      }
      return;
    }
    const deepestFirst = [...this.steps.values()]
      .sort((a, b) => a.path.length - b.path.length || a.seq - b.seq)
      .reverse()
      .map((s) => String(s.data.id));
    for (const id of deepestFirst) {
      const step = this.steps.get(id);
      if (!step) continue;
      step.data = { ...step.data, state: "canceled" };
      const snap = this.stepActivity(id);
      if (snap) out.push(snap);
      if (step.subOpen) out.push(Projector.canceledSubagent(step.sub));
      step.subOpen = false;
    }
    this.steps.clear();
  }

  /** A step of the agent's work: its `vymalo.step` activity and, for a sub-agent step, a subagent of its own. */
  private onAgentStep(e: Event, wasOpen: boolean, out: Ev[]) {
    const d = e.data;
    const id = str(d.id) ?? "";
    const phase = str(d.phase);
    const state = str(d.state) ?? "running";
    const ended = state === "completed" || state === "failed" || state === "canceled";
    const path = Array.isArray(d.path) ? d.path.map(String) : [];
    // a step is the sign that the agent works
    const movedToWorking = this.state !== "working";
    this.state = "working";
    this.ensureInvocation(e, out);
    const known = this.steps.get(id);
    if (known && phase !== "start") {
      // the icon is what the step first said when a later report leaves it out, and so is the input
      // (ADR 0030: logged once, with the start; the end says it again with the output)
      const input = d.input !== undefined ? d.input : known.data.input;
      known.data = {
        id,
        path,
        kind: d.kind,
        label: d.label,
        state,
        ...(d.icon !== undefined
          ? { icon: d.icon }
          : known.data.icon
            ? { icon: known.data.icon }
            : {}),
        ...(d.detail !== undefined ? { detail: d.detail } : {}),
        ...(input !== undefined ? { input } : {}),
        ...(d.output !== undefined ? { output: d.output } : {}),
        ...(d.ioDropped || known.data.ioDropped ? { ioDropped: true } : {}),
      };
      known.actor = e.actor;
    } else {
      // a step that starts again, or one the projection never saw start: a new run of it
      if (known?.subOpen) out.push(Projector.canceledSubagent(known.sub));
      this.steps.delete(id);
      const step: StepView = {
        seq: e.seq,
        data: {
          id,
          path,
          kind: d.kind,
          label: d.label,
          state,
          ...(d.icon !== undefined ? { icon: d.icon } : {}),
          ...(d.detail !== undefined ? { detail: d.detail } : {}),
          ...(d.input !== undefined ? { input: d.input } : {}),
          ...(d.output !== undefined ? { output: d.output } : {}),
          ...(d.ioDropped ? { ioDropped: true } : {}),
        },
        startedAt: e.at,
        actor: e.actor,
        path,
        subOpen: false,
      };
      if (d.kind === "subagent" && !ended) {
        step.sub = `sub-step-${e.seq}`;
        step.parent = this.enclosingRun(path);
        out.push(this.stepStarted(step));
        step.subOpen = true;
      }
      this.steps.set(id, step);
    }
    const snap = this.stepActivity(id);
    if (snap) out.push(snap);
    const step = this.steps.get(id);
    if (ended && step) {
      if (step.subOpen) {
        // whatever still runs under it ends first, deepest first: nesting stays whole
        for (const other of this.openStepSubagents(false)) {
          const child = this.steps.get(other);
          if (!child || other === id || !child.path.includes(id)) continue;
          child.subOpen = false;
          out.push(Projector.canceledSubagent(child.sub));
        }
        if (state === "failed") {
          out.push({
            type: "SUBAGENT_ERROR",
            subagentRunId: step.sub,
            message: str(d.detail) ?? `${String(d.label)} failed`,
            code: "step_failed",
          });
        } else if (state === "canceled") {
          out.push(Projector.canceledSubagent(step.sub));
        } else {
          out.push({ type: "SUBAGENT_FINISHED", subagentRunId: step.sub });
        }
      }
      this.steps.delete(id);
    }
    if (movedToWorking && wasOpen) out.push(this.snapshot());
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
    // a rename (or a description) says what the thread is in and changes none of it
    if (e.kind !== "thread_titled" && e.kind !== "thread_described") {
      this.interrupt = null;
      this.failure = null;
      this.state =
        e.kind === "agent_step" || (e.kind === "agent_status" && e.data.status === "working")
          ? "working"
          : "queued";
    }
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
    this.lastFinal = undefined;
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
      // every `vymalo.*` activity says when its event happened
      content: { ...content, at: e.at },
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
    /** What an agent's words are for, when the log says (ADR 0031): members of the START's metadata. */
    purpose?: { purpose?: string; via?: string },
  ): Ev[] {
    const attr = sub ? { subagentRunId: sub } : {};
    return [
      {
        type: "TEXT_MESSAGE_START",
        messageId,
        role,
        ...(role === "assistant" ? { name: e.actor.name } : {}),
        ...attr,
        metadata: {
          ...actorMeta(e),
          ...(purpose?.purpose ? { "vymalo.purpose": purpose.purpose } : {}),
          ...(purpose?.via ? { "vymalo.via": purpose.via } : {}),
        },
      },
      { type: "TEXT_MESSAGE_CONTENT", messageId, delta: text, ...attr },
      { type: "TEXT_MESSAGE_END", messageId, ...attr },
    ];
  }

  /**
   * The event that ends the open run, by the state the thread stands in: the wait again, the
   * failure, cancelled or success (the real projection's `close_run`). `thread_state` ends the
   * run with it, and so does a rename that opened a run of its own.
   */
  private runEnd(): Ev {
    const threadId = this.info.threadId;
    const runId = this.run?.runId ?? "";
    if (this.state === "blocked" && this.interrupt && !this.lastWasError) {
      const i = this.interrupt;
      return {
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
      };
    }
    if (this.state === "blocked" || this.state === "failed") {
      const f = this.failure ?? { message: "the agent failed", code: "agent_failed" };
      return {
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
      };
    }
    if (this.state === "cancelled") {
      return { type: "RUN_FINISHED", threadId, runId, outcome: { type: "cancelled" } };
    }
    return { type: "RUN_FINISHED", threadId, runId, outcome: { type: "success" } };
  }

  /** Frames of one log event. `id` (the seq) goes on the last frame, unless a message is open. */
  apply(e: Event, audience: Audience = {}): Frame[] {
    // The UI's catalog is not part of the transcript: the ledger moves and nothing is said (no
    // frame, so no resume point), whether or not a run is open.
    if (e.kind === "ui_catalog") {
      const ref = catalogRefOf(e.data);
      if (ref) this.catalog.observe(ref);
      return [];
    }
    const out: Ev[] = [];
    this.now = e.at;
    if (e.kind === "thread_forked") {
      this.onThreadForked(e, out);
      return out.map((event, i) => (i === out.length - 1 ? { id: e.seq, event } : { event }));
    }
    // the set of MCP servers is part of every snapshot, and a change is a card of its own
    if (e.kind === "tools_attached" || e.kind === "tools_detached") {
      this.onTools(e, out);
      return out.map((event, i) => (i === out.length - 1 ? { id: e.seq, event } : { event }));
    }
    // the title is part of every snapshot: a rename's own say it
    if (e.kind === "thread_titled")
      this.info = { ...this.info, title: str(e.data.title) ?? this.info.title };
    // and so is the description (ADR 0035); an empty one is a person clearing it
    if (e.kind === "thread_described")
      this.info = { ...this.info, description: str(e.data.description) || undefined };
    // a message on a finished thread starts the next job; so does a bare `job_started` (a
    // message redelivered to the agent), whose run it opens
    const finished = this.state === "done" || this.state === "failed" || this.state === "cancelled";
    const jobStart = e.kind === "job_started" ? Number(e.data.job) : undefined;
    const begunByMessage = jobStart !== undefined && jobStart === this.jobNumber;
    if (!this.run && finished && e.kind === "user_message") this.beginJob(this.jobNumber + 1);
    if (jobStart !== undefined && jobStart !== this.jobNumber) this.beginJob(jobStart);
    const wasOpen = this.run !== null;
    if (!this.run && e.kind !== "user_message" && e.kind !== "thread_state") this.openRun(e, out);
    switch (e.kind) {
      case "job_started": {
        out.push({
          type: "ACTIVITY_SNAPSHOT",
          messageId: `job-${jobStart}`,
          activityType: "vymalo.job",
          content: { job: jobStart, at: e.at },
          metadata: actorMeta(e),
        });
        if (!begunByMessage && wasOpen) out.push(this.snapshot());
        break;
      }
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
        if (final) this.lastFinal = text;
        const attr = { subagentRunId: inv.id };
        // what the words are for, when the log says; no member when it does not (ADR 0031)
        const purpose = {
          ...(e.data.purpose === "working" || e.data.purpose === "answer"
            ? { purpose: e.data.purpose }
            : {}),
          ...(e.data.via === "turn_output" ? { via: e.data.via } : {}),
        };
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
            out.push(...this.textTriad(e, id, text, "assistant", inv.id, purpose));
            this.said.add(`${id}\0${text}`);
          } else {
            const [start, content] = this.textTriad(e, id, text, "assistant", inv.id, purpose);
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
      case "agent_step": {
        this.onAgentStep(e, wasOpen, out);
        break;
      }
      case "agent_status": {
        const inv = this.ensureInvocation(e, out);
        const status = str(e.data.status) ?? "working";
        const detail = str(e.data.detail);
        // what the agent says when it finishes or asks is its answer: an assistant message
        // (`st-<seq>`), not a detail of the status, unless it said exactly that already
        const speaks =
          status === "completed" || status === "input_required" || status === "auth_required";
        if (
          speaks &&
          detail !== undefined &&
          detail.trim() !== "" &&
          this.lastFinal?.trim() !== detail.trim()
        ) {
          if (this.openText) {
            out.push({
              type: "TEXT_MESSAGE_END",
              messageId: this.openText.id,
              subagentRunId: inv.id,
            });
            this.openText = null;
          }
          out.push(...this.textTriad(e, `st-${e.seq}`, detail, "assistant", inv.id));
        }
        out.push(
          this.activity(
            e,
            "vymalo.status",
            { status, ...(detail !== undefined && !speaks ? { detail } : {}) },
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
          this.closeSteps("suspended", out);
          out.push({
            type: "SUBAGENT_FINISHED",
            subagentRunId: inv.id,
            outcome: { type: "suspended", interruptIds: [id] },
          });
          this.suspended = inv;
          this.invocation = null;
        } else if (status === "completed") {
          this.closeSteps("canceled", out);
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
          this.closeSteps("canceled", out);
          out.push({
            type: "SUBAGENT_ERROR",
            subagentRunId: inv.id,
            message,
            code: "agent_failed",
          });
          this.failure = { message, code: "agent_failed" };
          this.invocation = null;
        } else if (status === "canceled") {
          this.closeSteps("canceled", out);
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
        const { name, mimeType, uri, text, file } = e.data;
        // what is verified is what the agent had pushed when it finished
        const pushed = pushedCommit(name, uri, text);
        if (this.job() && this.state !== "verifying" && pushed !== undefined) this.sha = pushed;
        out.push(
          this.activity(
            e,
            "vymalo.artifact",
            {
              ...typedArtifact(name, uri, text, file, mimeType, this.info.threadId),
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
          content: { ...e.data, at: e.at },
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
        content.at = e.at;
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
          messageId:
            this.jobNumber > 1
              ? `rework-j${this.jobNumber}-${this.attempt}`
              : `rework-${this.attempt}`,
          activityType: "vymalo.rework",
          content: { ...e.data, at: e.at },
          replace: true,
          metadata: actorMeta(e),
        });
        if (!this.invocation) {
          const actor = this.lastAgent ?? {
            type: "agent" as const,
            name: this.info.target.agentId,
          };
          this.invocation = { id: `sub-${e.seq}`, event: { ...e, actor } };
          this.lastFinal = undefined;
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
          this.closeSteps("canceled", out);
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
      // A person renamed the thread (the real projection's `on_thread_titled`), or the thread was
      // described (`on_thread_described`, ADR 0035): inside a run a snapshot with the new title or
      // description; outside any, a run of its own that holds the snapshot (said by `openRun`) and
      // ends as the thread's state ends a run, which the web drops as material-less
      case "thread_titled":
      case "thread_described": {
        if (wasOpen) out.push(this.snapshot());
        else if (
          this.state !== "queued" &&
          this.state !== "working" &&
          this.state !== "verifying"
        ) {
          out.push(this.runEnd());
          this.run = null;
          this.invocation = null;
        }
        break;
      }
      case "thread_state": {
        if (!this.run) this.openRun(e, out);
        this.state = e.data.state as ThreadState;
        // a verifier still waited for when the run ends never answered (a timeout, a cancel, or
        // another source decided the round); the run's success is its pass
        this.closeVerifier(this.state === "done" ? { passed: true } : "abandoned", out);
        out.push(this.snapshot());
        out.push(this.runEnd());
        this.run = null;
        this.invocation = null;
        // a thread that waits or failed says it again when a rename closes a run of its own
        this.interrupt = this.state === "blocked" ? this.interrupt : null;
        this.failure = this.state === "blocked" || this.state === "failed" ? this.failure : null;
        break;
      }
    }
    return out.map((event, i) => {
      const last = i === out.length - 1;
      return last && !this.openText ? { id: e.seq, event } : { event };
    });
  }
}
