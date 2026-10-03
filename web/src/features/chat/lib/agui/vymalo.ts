import type { UiCatalogRef } from "@/features/chat/lib/a2ui/catalog";
import { type Answer, readAnswers } from "@/features/chat/lib/a2ui/choices";
import { safeHttpUrl } from "@/features/chat/lib/a2ui/url";
import { AGENT_STATUSES, type AgentStatus, type ApiActor } from "@/lib/api/types";

/**
 * The `vymalo.*` vocabulary of docs/api/agui.md: activity types, the actor metadata key and the
 * release-channels extension URI. A client that knows none of them still shows conforming runs.
 */
export const ACTIVITY = {
  status: "vymalo.status",
  artifact: "vymalo.artifact",
  error: "vymalo.error",
  action: "vymalo.action",
  /** One source of the verification gate answered for one attempt (ADR 0018). */
  check: "vymalo.check",
  /** The gate failed and the agent is sent back to work (ADR 0018). */
  rework: "vymalo.rework",
  /** A message on a finished thread started the thread's next job (ADR 0020): `{job}`, from 2. */
  job: "vymalo.job",
  /**
   * The thread began as a copy of another (ADR 0029): where the copy ends. A run of its own; the
   * chat draws it as a divider.
   */
  fork: "vymalo.fork",
  /** A CI system reported a check on a commit (ADR 0017), whether or not the gate counted it. */
  ci: "vymalo.ci",
  /**
   * MCP servers were attached to the thread, or detached from it (ADR 0024): ids only. The chat
   * draws one muted line for it, naming the servers from the deployment's list.
   */
  tools: "vymalo.tools",
  /**
   * A step of the agent's work (ADR 0025, steps/v1): the same activity says the step again at each
   * of its events, under one message id, and its `path` places it in the tree.
   */
  step: "vymalo.step",
  /**
   * An agent the thread's agent asked (ADR 0026, `ask_agent`): said again under one message id,
   * `ask-<n>`, when the ask ends. The step tree draws it as a line "Asked <agent>" under the step
   * (or the ask) that asked.
   */
  ask: "vymalo.ask",
  /**
   * An A2UI surface, as `ThreadAgent` hands it to the runtime. On the wire it is
   * `a2ui-surface` ({@link A2UI_SURFACE}); see `thread-agent.ts` for why it is renamed.
   */
  surface: "vymalo.a2ui-surface",
} as const;

/** The ecosystem's activity type of an A2UI surface (ADR 0013), the one the orchestrator sends. */
export const A2UI_SURFACE = "a2ui-surface";

/** `metadata["vymalo.actor"]` of an attributed event: `{type, name, revision?}`. */
export const ACTOR_KEY = "vymalo.actor";

/**
 * ADR 0024: `forwardedProps["vymalo.tools"]` on the run that creates a thread: the ids of the MCP
 * servers to attach to it, committed with the first message. Ignored by a run on a thread that
 * exists (the set is changed with `PUT /api/threads/{id}/tools`).
 */
export const TOOLS_PROP = "vymalo.tools";

/**
 * ADR 0036: `forwardedProps["vymalo.send"]` on a run that carries a message while a run is open:
 * how the message is delivered. `steer` is Send (the agent reads it at its next step, or after its
 * turn), `interrupt` is Stop and send (the running task is cancelled and the message starts the
 * next job). Without it a second run on an open thread is a 409.
 */
export const SEND_PROP = "vymalo.send";

export type SendMode = "steer" | "interrupt";

/**
 * `metadata["vymalo.delivery"]` of a user message's `START`: `steer` or `interrupt` when the core
 * logged the message that way (it was sent while the agent worked). Absent otherwise, and in every
 * log written before ADR 0036; anything else is no delivery.
 */
export const DELIVERY_KEY = "vymalo.delivery";

export const parseDelivery = (value: unknown): SendMode | undefined =>
  value === "steer" || value === "interrupt" ? value : undefined;

/**
 * `forwardedProps["vymalo.mentions"]` of a run that carries a message (ADR 0026,
 * docs/api/agui.md "Mentions"): the agents the message mentions, `[{agentId, label, start, end,
 * cardUrl?}]`, offsets in UTF-16 code units. Checked by the orchestrator before anything is written
 * (400, 422, 503); `features/mentions` builds it.
 */
export const MENTIONS_PROP = "vymalo.mentions";

/** `metadata["vymalo.mentions"]` of a user message's `START`: the same references, as the log kept them. */
export const MENTIONS_KEY = "vymalo.mentions";

/** The A2A extension an agent lists when it is told who a message mentioned (docs/api/mentions-v1.md). */
export const MENTIONS_URI = "https://agents.vymalo.com/a2a/extensions/mentions/v1";

/** ADR 0008: the release travels in `forwardedProps` under the extension URI. */
export const RELEASE_CHANNELS_URI = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";

/**
 * The data part name the runtime gives an `ACTIVITY_SNAPSHOT` of `activityType`:
 * `@assistant-ui/react-ag-ui` renders every activity but `a2ui-surface` and `mcp-apps` as the
 * data part `agui-activity/<activityType>`.
 */
export const activityPartName = (activityType: string) => `agui-activity/${activityType}`;

/**
 * The marker part `ThreadAgent` puts in front of an invocation's output (a `CUSTOM` event, which
 * the runtime turns into a data part of this name). It carries who ran and which revision, so a
 * text bubble, which the runtime reduces to bare text, can still show its actor.
 */
export const ACTOR_PART = "vymalo.actor";

/**
 * `metadata["vymalo.purpose"]` of a text message's `START` (ADR 0031): what the agent's words are
 * for, `"working"` (said while it works, before a tool call) or `"answer"` (what its turn ends
 * with). Absent for text the log does not mark (a plain A2A agent, the status words, an older log).
 */
export const PURPOSE_KEY = "vymalo.purpose";

/**
 * The marker part `ThreadAgent` puts right before the text of a message that says its purpose (a
 * `CUSTOM` event, which the runtime turns into a data part of this name): the runtime reduces a
 * text message to bare text and drops its metadata, so the mark rides in front of the text it
 * belongs to. Like the actor marker, it is read by the turn and drawn as nothing.
 */
export const PURPOSE_PART = "vymalo.purpose";

export type TextPurpose = "working" | "answer";

/** `"working"` or `"answer"`; anything else (a newer word, a wrong type) is no purpose at all. */
export const parsePurpose = (value: unknown): TextPurpose | undefined =>
  value === "working" || value === "answer" ? value : undefined;

/**
 * The contents of the activities, as the renderers read them. `actor` is not on the wire in the
 * content: the runtime drops an event's `metadata`, so `ThreadAgent` folds
 * `metadata["vymalo.actor"]` into the content it hands over.
 */
export type WithActor<T> = T & {
  actor?: ApiActor;
  /** When the event happened (RFC 3339, `Event.at`); every `vymalo.*` activity carries it. */
  at?: string;
};
export type StatusContent = WithActor<{ status: AgentStatus; detail?: string }>;
export const ARTIFACT_KINDS = ["branch", "checks", "pull_request", "file"] as const;
export type ArtifactKind = (typeof ARTIFACT_KINDS)[number];

/** What a person can look at in a kept file without downloading it (`preview` of a `file`). */
export type FilePreview = "image" | "text";

/**
 * An artifact as the projection typed it (docs/api/agui.md, "Typed artifacts"): `kind` and the
 * fields its card needs. An artifact of an older orchestrator, or of a kind this UI does not know,
 * is a `file`. Every string comes from the agent: untrusted text; `url` is kept only when it is an
 * absolute `https` link.
 */
export type ArtifactContent = WithActor<{
  kind: ArtifactKind;
  name: string;
  mimeType?: string;
  uri?: string;
  text?: string;
  repository?: string;
  branch?: string;
  sha?: string;
  shortSha?: string;
  url?: string;
  number?: number;
  passed?: boolean;
  /**
   * A file the artifact store kept (ADR 0032): where the API serves it (`href`, always
   * `/api/threads/<thread>/artifacts/<sha256>`), its hash, size in bytes, the agent's file name
   * (untrusted text) and how it can be previewed (`null`: an attachment only). Present together,
   * and only for a `kind: "file"` whose `href` says the same hash as `sha256`; a file that was not
   * kept has none of them.
   */
  href?: string;
  sha256?: string;
  size?: number;
  filename?: string;
  preview?: FilePreview | null;
}>;
export type ErrorContent = WithActor<{ message: string; retryable: boolean }>;
export type ActionContent = WithActor<{
  surfaceId: string;
  name: string;
  sourceComponentId?: string;
  context?: Record<string, unknown>;
}>;
export const CHECK_STATUSES = ["pending", "passed", "failed"] as const;
export type CheckStatus = (typeof CHECK_STATUSES)[number];

/**
 * One source of the gate for one attempt. The strings (`summary`, `findings`, `name`, `commit`) come
 * from a tool, a CI provider or a reviewer: untrusted text, drawn as text and nothing else.
 * `source` is whatever the orchestrator named (`ci`, `agent_checks`, `verifier` today).
 */
export type CheckContent = WithActor<{
  source: string;
  attempt: number;
  status: CheckStatus;
  name?: string;
  commit?: string;
  summary?: string;
  /** The answer belongs to a verification that is no longer the current one; it decided nothing. */
  stale: boolean;
  findings: string[];
}>;

/**
 * A CI report (ADR 0017). `name`, `branch` and `summary` are written by whoever runs the CI:
 * untrusted text, drawn as text and nothing else. `conclusion` is a string on purpose: the set is
 * closed today (`docs/api/webhooks.md#conclusions`, `startup_failure` too), a newer orchestrator may
 * add one, and `passed` says how it counts. `url` is here only when it is an http(s) link.
 */
export type CiContent = WithActor<{
  name: string;
  conclusion: string;
  passed: boolean;
  sha: string;
  shortSha: string;
  provider: string;
  repository: string;
  branch?: string;
  url?: string;
  summary?: string;
}>;

/** What one failed source said, as the rework carries it. */
export type ReworkFindings = { source: string; findings: string[] };

export type ReworkContent = WithActor<{
  /** The attempt that starts now. */
  attempt: number;
  maxAttempts: number;
  findings: ReworkFindings[];
}>;

export const STEP_KINDS = ["subagent", "tool", "command", "message"] as const;
export type StepKind = (typeof STEP_KINDS)[number];
export const STEP_STATES = ["running", "waiting", "completed", "failed", "canceled"] as const;
export type StepState = (typeof STEP_STATES)[number];
/** The icon vocabulary of steps/v1 (docs/api/steps-v1.md): anything else is ignored. */
export const STEP_ICONS = [
  "agent",
  "read",
  "edit",
  "delete",
  "move",
  "search",
  "execute",
  "think",
  "fetch",
  "web",
  "git",
  "test",
  "file",
  "tool",
] as const;
export type StepIcon = (typeof STEP_ICONS)[number];

/** The id of an MCP server the deployment offers (ADR 0024, `ToolServer.id`). */
export const SERVER_ID = /^[a-z0-9][a-z0-9-]{0,30}$/;

/**
 * `icon: "mcp-server:<id>"` of a step the orchestrator reports for a call of an attached MCP
 * server (docs/api/thread-tools-v1.md, "The step of a call"): the server whose icon the step shows.
 * An id that is not a server id is no server.
 */
export const MCP_ICON_PREFIX = "mcp-server:";

export function serverOfIcon(icon: string | undefined): string | undefined {
  if (!icon?.startsWith(MCP_ICON_PREFIX)) return undefined;
  const id = icon.slice(MCP_ICON_PREFIX.length);
  return SERVER_ID.test(id) ? id : undefined;
}

/**
 * A step of the agent's work as it stands (`vymalo.step`, docs/api/agui.md "Nested steps"). `id` is
 * unique in the thread and `path` the ids of the steps it runs under, outermost first. `label` and
 * `detail` come from an agent: untrusted text, drawn as text. `at` is the time of this event and
 * `startedAt` the time of the step's first.
 */
export type StepContent = WithActor<{
  id: string;
  path: string[];
  kind: StepKind;
  label: string;
  state: StepState;
  icon?: StepIcon;
  /**
   * The MCP server the step is a call of (`icon: "mcp-server:<id>"`), whose own icon the step shows
   * in place of the vocabulary's glyph. The id only: the name and the icon come from the
   * deployment's list (`GET /api/tool-servers`).
   */
  server?: string;
  detail?: string;
  startedAt?: string;
  input?: StepInput;
  output?: StepOutput;
  ioDropped?: true;
}>;

/**
 * What a tool was called with (ADR 0030): a JSON object with any keys, from an agent, so untrusted:
 * it is drawn as text and never read for meaning. The orchestrator cut it (strings at 512
 * characters, 4096 bytes in all) and redacted credentials (`"[redacted]"`); one that was bigger is
 * exactly `{"_cut": true, "bytes": n}` (`inputCut`).
 */
export type StepInput = Record<string, unknown>;

/**
 * What a tool returned, on its step's end (ADR 0030). `text` is at most 8192 bytes: of a longer one
 * only the head and the tail are kept, with a line between that says how much is not
 * (`truncated`, and `bytes`, the size of all of it). `error`: `text` is the error the tool returned.
 */
export type StepOutput = { text: string; truncated?: true; bytes?: number; error?: true };

/** `{_cut: true, bytes}`: an input that was too big to keep, and how big it was. */
export function inputCut(input: StepInput): { bytes: number } | null {
  const keys = Object.keys(input);
  if (input._cut !== true || keys.length > 2 || (keys.length === 2 && !("bytes" in input))) {
    return null;
  }
  return { bytes: typeof input.bytes === "number" && input.bytes >= 0 ? input.bytes : 0 };
}

export const ASK_STATES = [
  "running",
  "completed",
  "input_required",
  "auth_required",
  "failed",
  "rejected",
  "canceled",
  "timed_out",
] as const;
export type AskState = (typeof ASK_STATES)[number];

/** What an asked agent handed back, by name; never its bytes. */
export type AskArtifact = { name: string; uri?: string; mimeType?: string };

/**
 * `vymalo.ask` (docs/api/agui.md, "Asked agents as subagents"): what was asked of which agent, who
 * asked, and how it stands. `agent` is an agent id (the name is the agent list's); `by` is `main`
 * (the thread's agent) or `ask:<n>`; `text`, `answer`, `question` and `error` are agents' words:
 * untrusted text, drawn as text.
 */
export type AskContent = WithActor<{
  ask: number;
  agent: string;
  by: string;
  depth: number;
  text: string;
  stepId: string;
  /** The step of the asking agent the ask runs under, as the agent reported it. */
  parentStepId?: string;
  state: AskState;
  startedAt?: string;
  answer?: string;
  question?: string;
  artifacts?: AskArtifact[];
  error?: string;
}>;

/** `job` of a `STATE_SNAPSHOT` and of `Thread` (chat-api.yaml, `ThreadJob`): only under a gate. */
export type JobView = {
  /** Which job of the thread this is; present from job 2 (ADR 0020), absent means job 1. */
  number?: number;
  attempt: number;
  maxAttempts: number;
  gate: string[];
  sha?: string;
};

/**
 * A surface: the operations exactly as the orchestrator sent them (untrusted, read by
 * `lib/a2ui/prepare.ts` and nothing else) and `surface`, the activity message id that identifies
 * the surface across updates. `actor` is the only label a surface gets.
 */
export type SurfaceContent = WithActor<{ a2ui_operations?: unknown; surface: string }>;

// ---- reading the contents -------------------------------------------------------------------
//
// A part's `data` is whatever the agent's activity carried: the renderers read it through these
// guards and show nothing for a shape they do not know (the UI renders events, never guesses).

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

const readActor = (v: unknown): ApiActor | undefined => {
  if (!isRecord(v)) return undefined;
  const name = str(v.name);
  const type = str(v.type);
  if (!name || (type !== "user" && type !== "agent" && type !== "system")) return undefined;
  const revision = str(v.revision);
  return { type, name, ...(revision ? { revision } : {}) };
};

export const parseActor = readActor;

/**
 * The `runId` `ThreadAgent` puts in the actor marker part: the run of the log the turn is, which
 * is how a turn finds where it ends in the log (`ThreadAgent.endOfRun`). Undefined for a marker
 * of an older build or a part that is not one.
 */
export const parseActorRun = (v: unknown): string | undefined =>
  isRecord(v) ? str(v.runId) : undefined;

/**
 * `vymalo.tools` (docs/api/agui.md, "Attaching MCP servers"): the ids that came, or the ids that
 * went, never both and never a name or a URL. A payload with neither is nothing.
 */
export type ToolsContent = WithActor<{ attached?: string[]; detached?: string[] }>;

const serverIds = (v: unknown): string[] =>
  Array.isArray(v)
    ? v.filter((id): id is string => typeof id === "string" && SERVER_ID.test(id))
    : [];

/**
 * `thread.tools` of a snapshot or of the thread resource: the ids of the servers attached, or
 * undefined when there are none (the member is absent then; so is one that is not a list of ids).
 */
export const parseToolIds = (v: unknown): string[] | undefined => {
  const ids = serverIds(v);
  return ids.length > 0 ? ids : undefined;
};

export function parseTools(v: unknown): ToolsContent | null {
  if (!isRecord(v)) return null;
  const attached = serverIds(v.attached);
  const detached = serverIds(v.detached);
  if (attached.length === 0 && detached.length === 0) return null;
  const actor = readActor(v.actor);
  return {
    ...(attached.length > 0 ? { attached } : {}),
    ...(detached.length > 0 ? { detached } : {}),
    ...(actor ? { actor } : {}),
    ...readAt(v),
  };
}

/** `vymalo.fork` (docs/api/agui.md, "Forks"): what the thread was made from, and how. */
export type ForkContent = WithActor<{
  from: { threadId: string; seq: number };
  kind: "fork" | "edit";
  title: string;
  target: { agentId: string; release?: string };
}>;

export function parseFork(v: unknown): ForkContent | null {
  if (!isRecord(v) || !isRecord(v.from) || !isRecord(v.target)) return null;
  const threadId = str(v.from.threadId);
  const seq = v.from.seq;
  const agentId = str(v.target.agentId);
  const kind = v.kind;
  if (!threadId || typeof seq !== "number" || !Number.isInteger(seq) || seq < 0 || !agentId) {
    return null;
  }
  if (kind !== "fork" && kind !== "edit") return null;
  const release = str(v.target.release);
  const actor = readActor(v.actor);
  return {
    from: { threadId, seq },
    kind,
    title: str(v.title) ?? "",
    target: { agentId, ...(release ? { release } : {}) },
    ...(actor ? { actor } : {}),
    ...readAt(v),
  };
}

/** `{at}` when `v.at` is a time a `Date` can read, else nothing. */
const readAt = (v: Record<string, unknown>): { at?: string } => {
  const at = str(v.at);
  return at && !Number.isNaN(Date.parse(at)) ? { at } : {};
};

export function parseStatus(v: unknown): StatusContent | null {
  if (!isRecord(v)) return null;
  const status = str(v.status);
  if (!status || !(AGENT_STATUSES as readonly string[]).includes(status)) return null;
  const detail = str(v.detail);
  const actor = readActor(v.actor);
  return {
    status: status as AgentStatus,
    ...(detail !== undefined ? { detail } : {}),
    ...(actor ? { actor } : {}),
    ...readAt(v),
  };
}

/** Where the API serves a thread's file: the one place a file is ever fetched from. */
const FILE_HREF = /^\/api\/threads\/[A-Za-z0-9_-]{1,64}\/artifacts\/([0-9a-f]{64})$/;

/**
 * The fields of a kept file (`href`, `sha256`, `size`, `filename?`, `preview`), or nothing: a
 * payload whose `href` is not the API's route, whose hash is not the one in the `href` or whose
 * size is not a count of bytes is not a file this app offers. The agent's `uri` is never a source.
 */
function readKeptFile(v: Record<string, unknown>): Partial<ArtifactContent> {
  const href = str(v.href);
  const sha256 = str(v.sha256);
  const size = v.size;
  if (href === undefined || sha256 === undefined) return {};
  if (FILE_HREF.exec(href)?.[1] !== sha256) return {};
  if (typeof size !== "number" || !Number.isSafeInteger(size) || size < 0) return {};
  const filename = str(v.filename);
  return {
    href,
    sha256,
    size,
    ...(filename ? { filename } : {}),
    preview: v.preview === "image" || v.preview === "text" ? v.preview : null,
  };
}

export function parseArtifact(v: unknown): ArtifactContent | null {
  if (!isRecord(v)) return null;
  const name = str(v.name);
  if (name === undefined) return null;
  const kind = str(v.kind);
  const mimeType = str(v.mimeType);
  const uri = str(v.uri);
  const text = str(v.text);
  const repository = str(v.repository);
  const branch = str(v.branch);
  const sha = str(v.sha);
  const shortSha = str(v.shortSha);
  const link = safeHttpUrl(v.url);
  const url = link?.startsWith("https://") ? link : undefined;
  const number = positiveInt(v.number);
  const actor = readActor(v.actor);
  const resolved =
    kind && (ARTIFACT_KINDS as readonly string[]).includes(kind) ? (kind as ArtifactKind) : "file";
  return {
    kind: resolved,
    name,
    ...(mimeType !== undefined ? { mimeType } : {}),
    ...(uri !== undefined ? { uri } : {}),
    ...(text !== undefined ? { text } : {}),
    ...(repository ? { repository } : {}),
    ...(branch ? { branch } : {}),
    ...(sha ? { sha } : {}),
    ...(shortSha ? { shortSha } : {}),
    ...(url ? { url } : {}),
    ...(number !== undefined ? { number } : {}),
    ...(typeof v.passed === "boolean" ? { passed: v.passed } : {}),
    ...(resolved === "file" ? readKeptFile(v) : {}),
    ...(actor ? { actor } : {}),
    ...readAt(v),
  };
}

export function parseError(v: unknown): ErrorContent | null {
  if (!isRecord(v)) return null;
  const message = str(v.message);
  if (message === undefined) return null;
  const actor = readActor(v.actor);
  return { message, retryable: v.retryable === true, ...(actor ? { actor } : {}), ...readAt(v) };
}

export function parseAction(v: unknown): ActionContent | null {
  if (!isRecord(v)) return null;
  const surfaceId = str(v.surfaceId);
  const name = str(v.name);
  if (surfaceId === undefined || name === undefined) return null;
  const sourceComponentId = str(v.sourceComponentId);
  const actor = readActor(v.actor);
  return {
    surfaceId,
    name,
    ...(sourceComponentId !== undefined ? { sourceComponentId } : {}),
    ...(isRecord(v.context) ? { context: v.context } : {}),
    ...(actor ? { actor } : {}),
    ...readAt(v),
  };
}

/**
 * An action that answers a `Choices` (docs/api/ui-catalog-v1.md, "Choices answers"): a
 * `vymalo.action` whose `context.answers` is a list of `{id, values, other?}`. It is the person's
 * answer, shown as such, and not a step. Any other action is nothing here.
 */
export type AnswersContent = WithActor<{
  surfaceId: string;
  sourceComponentId?: string;
  answers: Answer[];
}>;

export function parseAnswers(v: unknown): AnswersContent | null {
  const action = parseAction(v);
  const answers = action ? readAnswers(action.context) : null;
  if (!action || !answers) return null;
  return {
    surfaceId: action.surfaceId,
    ...(action.sourceComponentId !== undefined
      ? { sourceComponentId: action.sourceComponentId }
      : {}),
    answers,
    ...(action.actor ? { actor: action.actor } : {}),
    ...(action.at ? { at: action.at } : {}),
  };
}

/** How many findings a card reads: the orchestrator sends at most 20 per source. */
const MAX_FINDINGS = 100;

const strings = (v: unknown): string[] =>
  Array.isArray(v)
    ? v.filter((x): x is string => typeof x === "string").slice(0, MAX_FINDINGS)
    : [];

const positiveInt = (v: unknown): number | undefined =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 1 ? v : undefined;

/** `vymalo.check`; a payload without a source, an attempt or a known status renders nothing. */
export function parseCheck(v: unknown): CheckContent | null {
  if (!isRecord(v)) return null;
  const source = str(v.source);
  const attempt = positiveInt(v.attempt);
  const status = str(v.status);
  if (!source || attempt === undefined) return null;
  if (!status || !(CHECK_STATUSES as readonly string[]).includes(status)) return null;
  const name = str(v.name);
  const commit = str(v.commit);
  const summary = str(v.summary);
  const actor = readActor(v.actor);
  return {
    source,
    attempt,
    status: status as CheckStatus,
    ...(name ? { name } : {}),
    ...(commit ? { commit } : {}),
    ...(summary ? { summary } : {}),
    stale: v.stale === true,
    findings: strings(v.findings),
    ...(actor ? { actor } : {}),
    ...readAt(v),
  };
}

/**
 * `vymalo.ci`; a payload without a name, a conclusion, whether it passed, a commit, a provider or a
 * repository renders nothing. A `url` that is not an absolute http(s) link is dropped here (the
 * projection checked it already, and the log is data), and the card checks it once more.
 */
export function parseCi(v: unknown): CiContent | null {
  if (!isRecord(v)) return null;
  const name = str(v.name);
  const conclusion = str(v.conclusion);
  const sha = str(v.sha);
  const shortSha = str(v.shortSha);
  const provider = str(v.provider);
  const repository = str(v.repository);
  if (!name || !conclusion || typeof v.passed !== "boolean") return null;
  if (!sha || !shortSha || !provider || !repository) return null;
  const branch = str(v.branch);
  const url = safeHttpUrl(v.url);
  const summary = str(v.summary);
  const actor = readActor(v.actor);
  return {
    name,
    conclusion,
    passed: v.passed,
    sha,
    shortSha,
    provider,
    repository,
    ...(branch ? { branch } : {}),
    ...(url ? { url } : {}),
    ...(summary ? { summary } : {}),
    ...(actor ? { actor } : {}),
    ...readAt(v),
  };
}

/** `vymalo.rework`; a payload without the attempt and the attempts there are renders nothing. */
export function parseRework(v: unknown): ReworkContent | null {
  if (!isRecord(v)) return null;
  const attempt = positiveInt(v.attempt);
  const maxAttempts = positiveInt(v.maxAttempts);
  if (attempt === undefined || maxAttempts === undefined) return null;
  const findings = Array.isArray(v.findings)
    ? v.findings.flatMap((f): ReworkFindings[] => {
        if (!isRecord(f)) return [];
        const source = str(f.source);
        return source ? [{ source, findings: strings(f.findings) }] : [];
      })
    : [];
  const actor = readActor(v.actor);
  return { attempt, maxAttempts, findings, ...(actor ? { actor } : {}), ...readAt(v) };
}

function readStepInput(v: unknown): StepInput | undefined {
  return isRecord(v) && Object.keys(v).length > 0 ? v : undefined;
}

function readStepOutput(v: unknown): StepOutput | undefined {
  if (!isRecord(v)) return undefined;
  const text = str(v.text);
  if (text === undefined) return undefined;
  const bytes = typeof v.bytes === "number" && Number.isInteger(v.bytes) && v.bytes >= 0;
  return {
    text,
    ...(v.truncated === true ? { truncated: true as const } : {}),
    ...(bytes ? { bytes: v.bytes as number } : {}),
    ...(v.error === true ? { error: true as const } : {}),
  };
}

/**
 * `vymalo.step`; a payload without an `id`, a `label` and a known `state` renders nothing (the
 * orchestrator's door checked it already, and the log is data). An unknown `kind` is a tool, an
 * icon outside the vocabulary is none, a `path` that is not a list of strings is the top level.
 * `input` and `output` (ADR 0030) are read leniently, as the orchestrator reads them: one of the
 * wrong shape is dropped and the step is kept.
 */
export function parseStep(v: unknown): StepContent | null {
  if (!isRecord(v)) return null;
  const id = str(v.id);
  const label = str(v.label);
  const state = str(v.state);
  if (!id || label === undefined || !state) return null;
  if (!(STEP_STATES as readonly string[]).includes(state)) return null;
  const kind = str(v.kind);
  const icon = str(v.icon);
  const detail = str(v.detail);
  const actor = readActor(v.actor);
  const startedAt = str(v.startedAt);
  const input = readStepInput(v.input);
  const output = readStepOutput(v.output);
  return {
    id,
    path: Array.isArray(v.path) ? v.path.filter((p): p is string => typeof p === "string") : [],
    kind: kind && (STEP_KINDS as readonly string[]).includes(kind) ? (kind as StepKind) : "tool",
    label,
    state: state as StepState,
    ...(icon && (STEP_ICONS as readonly string[]).includes(icon) ? { icon: icon as StepIcon } : {}),
    ...(serverOfIcon(icon) ? { server: serverOfIcon(icon) as string } : {}),
    ...(detail ? { detail } : {}),
    ...(startedAt && !Number.isNaN(Date.parse(startedAt)) ? { startedAt } : {}),
    ...(input ? { input } : {}),
    ...(output ? { output } : {}),
    ...(v.ioDropped === true ? { ioDropped: true as const } : {}),
    ...(actor ? { actor } : {}),
    ...readAt(v),
  };
}

/**
 * `vymalo.ask`; a payload without an ask number, an agent, a step id and a known `state` is
 * nothing. `by` that is not `main` or `ask:<n>` is the thread's agent. Artifacts without a name are
 * dropped, and only the name, the `uri` and the `mimeType` of one are kept.
 */
export function parseAsk(v: unknown): AskContent | null {
  if (!isRecord(v)) return null;
  const ask = positiveInt(v.ask);
  const agent = str(v.agent);
  const stepId = str(v.stepId);
  const state = str(v.state);
  if (ask === undefined || !agent || !stepId || !state) return null;
  if (!(ASK_STATES as readonly string[]).includes(state)) return null;
  const by = str(v.by);
  const parentStepId = str(v.parentStepId);
  const startedAt = str(v.startedAt);
  const answer = str(v.answer);
  const question = str(v.question);
  const error = str(v.error);
  const actor = readActor(v.actor);
  const artifacts = Array.isArray(v.artifacts)
    ? v.artifacts.flatMap((a): AskArtifact[] => {
        if (!isRecord(a)) return [];
        const name = str(a.name);
        if (!name) return [];
        const uri = str(a.uri);
        const mimeType = str(a.mimeType);
        return [{ name, ...(uri ? { uri } : {}), ...(mimeType ? { mimeType } : {}) }];
      })
    : [];
  return {
    ask,
    agent,
    by: by && /^(main|ask:[1-9][0-9]*)$/.test(by) ? by : "main",
    depth: positiveInt(v.depth) ?? 1,
    text: str(v.text) ?? "",
    stepId,
    ...(parentStepId ? { parentStepId } : {}),
    state: state as AskState,
    ...(startedAt && !Number.isNaN(Date.parse(startedAt)) ? { startedAt } : {}),
    ...(answer ? { answer } : {}),
    ...(question ? { question } : {}),
    ...(artifacts.length > 0 ? { artifacts } : {}),
    ...(error ? { error } : {}),
    ...(actor ? { actor } : {}),
    ...readAt(v),
  };
}

/** `job` of a snapshot or of the thread resource; null when it is absent or not a job. */
export function parseJob(v: unknown): JobView | null {
  if (!isRecord(v)) return null;
  const attempt = positiveInt(v.attempt);
  const maxAttempts = positiveInt(v.maxAttempts);
  if (attempt === undefined || maxAttempts === undefined) return null;
  const sha = str(v.sha);
  const number = positiveInt(v.number);
  return {
    ...(number !== undefined && number > 1 ? { number } : {}),
    attempt,
    maxAttempts,
    gate: Array.isArray(v.gate) ? v.gate.filter((x): x is string => typeof x === "string") : [],
    ...(sha ? { sha } : {}),
  };
}

/** The digest of a catalog: `sha256:` and 64 lowercase hex digits (what the orchestrator checks). */
export const DIGEST = /^sha256:[0-9a-f]{64}$/;

/**
 * `thread.uiCatalog` of a snapshot: the catalog the thread's agents are told about. Null when it
 * is absent or not a `{catalogId, version, digest}` this build can compare.
 */
export function parseUiCatalog(v: unknown): UiCatalogRef | null {
  if (!isRecord(v)) return null;
  const catalogId = str(v.catalogId);
  const version = positiveInt(v.version);
  const digest = str(v.digest);
  if (!catalogId || version === undefined || !digest || !DIGEST.test(digest)) return null;
  return { catalogId, version, digest };
}

export function parseSurface(v: unknown): SurfaceContent | null {
  if (!isRecord(v)) return null;
  const surface = str(v.surface);
  if (surface === undefined) return null;
  const actor = readActor(v.actor);
  return {
    surface,
    ...("a2ui_operations" in v ? { a2ui_operations: v.a2ui_operations } : {}),
    ...(actor ? { actor } : {}),
  };
}
