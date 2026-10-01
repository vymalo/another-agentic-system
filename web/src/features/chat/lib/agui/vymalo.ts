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
  /** A CI system reported a check on a commit (ADR 0017), whether or not the gate counted it. */
  ci: "vymalo.ci",
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
  return {
    kind:
      kind && (ARTIFACT_KINDS as readonly string[]).includes(kind)
        ? (kind as ArtifactKind)
        : "file",
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
    ...(actor ? { actor } : {}),
    ...readAt(v),
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
