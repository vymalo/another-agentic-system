import {
  ACTIVITY,
  ACTOR_PART,
  type ArtifactContent,
  activityPartName,
  type CheckContent,
  parseArtifact,
  parseStatus,
  type ReworkContent,
} from "@/features/chat/lib/agui/vymalo";
import { detectPullRequest } from "./artifact";
import { sourceLabel, truncate } from "./findings";

/*
 * How an agent's turn reads (web/DESIGN.md, "A turn"): its activities are steps in one compact
 * list, its words are prose, and a pull request or a file is a card after the words. These are the
 * pure decisions behind that; the components are in components/steps and components/cards.
 */

/** The data parts that are lines of the step list (or nothing at all, like the actor marker). */
const STEP_PARTS = new Set(
  [
    ACTIVITY.status,
    ACTIVITY.artifact,
    ACTIVITY.check,
    ACTIVITY.ci,
    ACTIVITY.rework,
    ACTIVITY.action,
    ACTIVITY.job,
  ].map(activityPartName),
);

/** What a part is, read from the `{type, name, data}` the runtime keeps. */
type PartLike = { type: string; name?: string; data?: unknown; text?: string };

/**
 * Whether a part belongs in the step list. A failed status is not a step: it is an error in the
 * flow of the turn. The actor marker goes with the steps so that it never splits a list.
 */
export function isStepPart(part: PartLike): boolean {
  if (part.type !== "data" || !part.name) return false;
  if (part.name === ACTOR_PART) return true;
  if (!STEP_PARTS.has(part.name)) return false;
  if (part.name === activityPartName(ACTIVITY.status)) {
    return parseStatus(part.data)?.status !== "failed";
  }
  return true;
}

/**
 * Whether a step part draws a line. The statuses that come with the agent's words (`completed`,
 * `input_required`) say nothing the words and the thread's state do not; the actor marker and the
 * job marker draw nothing.
 */
export function drawsStep(part: PartLike): boolean {
  if (!isStepPart(part) || part.name === ACTOR_PART) return false;
  if (part.name === activityPartName(ACTIVITY.job)) return false;
  if (part.name === activityPartName(ACTIVITY.status)) {
    const status = parseStatus(part.data)?.status;
    return status !== "completed" && status !== "input_required" && status !== undefined;
  }
  if (part.name === activityPartName(ACTIVITY.artifact)) return parseArtifact(part.data) !== null;
  return true;
}

/** The artifacts that are also a card after the agent's words: a pull request, a file. */
export function isCardArtifact(artifact: ArtifactContent): boolean {
  return artifact.kind === "pull_request" || artifact.kind === "file";
}

const COMMAND = /^\$\s+([\s\S]+)$/;

/** The command of a `working` detail written as `$ …` (a shell prompt), else nothing. */
export function commandOf(detail: string): string | undefined {
  const m = COMMAND.exec(detail.trim());
  return m?.[1]?.trim() || undefined;
}

/** `github.com/acme/demo` → `acme/demo`; a URL or an unknown shape is shortened, not parsed. */
export function shortRepository(repository: string): string {
  const bare = repository
    .replace(/^[a-z]+:\/\//i, "")
    .replace(/\.git$/, "")
    .replace(/\/+$/, "");
  const parts = bare.split("/");
  const short = parts.length >= 3 ? parts.slice(1).join("/") : bare;
  return truncate(short, 60).text;
}

/** What a `checks` artifact's JSON says besides `passed`: the summary and the findings, as text. */
export function checksPayload(text: string | undefined): { summary?: string; findings: string[] } {
  if (!text) return { findings: [] };
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return { findings: [] };
  }
  if (typeof value !== "object" || value === null) return { findings: [] };
  const v = value as Record<string, unknown>;
  const summary = typeof v.summary === "string" && v.summary.trim() ? v.summary : undefined;
  const findings = Array.isArray(v.findings)
    ? v.findings.filter((f): f is string => typeof f === "string").slice(0, 100)
    : [];
  return { ...(summary ? { summary } : {}), findings };
}

/** A pull request as its card and its step show it. */
export type PullRequestView = {
  href: string;
  /** `acme/demo#12`, `group/project!3`, or `#12` when the repository is not known. */
  label: string;
  number?: number;
  repository?: string;
  branch?: string;
  /** The title the agent gave it, when its payload has one (untrusted text). */
  title?: string;
  /** What the agent wrote with it that is not JSON (a file artifact's text). */
  note?: string;
};

const titleOf = (text: string | undefined): string | undefined => {
  if (!text) return undefined;
  try {
    const v = JSON.parse(text) as unknown;
    if (
      typeof v === "object" &&
      v !== null &&
      typeof (v as { title?: unknown }).title === "string"
    ) {
      const title = (v as { title: string }).title.trim();
      return title ? truncate(title, 200).text : undefined;
    }
  } catch {
    // not JSON: no title
  }
  return undefined;
};

/**
 * The pull request of an artifact: one the projection typed (`kind: pull_request`, its `url`
 * checked to be https), or a file whose link is a GitHub pull or GitLab merge request URL.
 */
export function pullRequestOf(artifact: ArtifactContent): PullRequestView | undefined {
  if (artifact.kind === "pull_request" && artifact.url) {
    const repository = artifact.repository ? shortRepository(artifact.repository) : undefined;
    const fromUrl = detectPullRequest(artifact.url);
    const label =
      repository && artifact.number !== undefined
        ? `${repository}#${artifact.number}`
        : (fromUrl?.label ??
          (artifact.number !== undefined ? `#${artifact.number}` : "pull request"));
    const title = titleOf(artifact.text);
    return {
      href: artifact.url,
      label,
      ...(artifact.number !== undefined ? { number: artifact.number } : {}),
      ...(repository ? { repository } : {}),
      ...(artifact.branch ? { branch: artifact.branch } : {}),
      ...(title ? { title } : {}),
    };
  }
  const detected = artifact.kind === "file" ? detectPullRequest(artifact.uri) : undefined;
  if (!detected) return undefined;
  const number = Number(/[#!](\d+)$/.exec(detected.label)?.[1]);
  return {
    href: detected.href,
    label: detected.label,
    ...(Number.isSafeInteger(number) ? { number } : {}),
    repository: detected.label.replace(/[#!]\d+$/, ""),
    ...(artifact.text?.trim() ? { note: artifact.text } : {}),
  };
}

/** The words of a step for a check of the gate (ADR 0018), by source and status. */
export function checkLabel(check: Pick<CheckContent, "source" | "status" | "stale">): string {
  const label = sourceLabel(check.source);
  if (check.stale) return `A late ${label} answer`;
  if (check.status === "pending") {
    if (check.source === "ci") return "Waiting for CI";
    if (check.source === "verifier") return "The verifier is reviewing the work";
    if (check.source === "agent_checks") return "Verifying the agent's checks";
    return `Waiting for ${label}`;
  }
  if (check.source === "verifier") {
    return check.status === "passed"
      ? "The verifier approved the work"
      : "The verifier found issues";
  }
  if (check.source === "agent_checks") {
    return check.status === "passed" ? "Verified the agent's checks" : "The agent's checks failed";
  }
  return check.status === "passed" ? `${label} passed` : `${label} failed`;
}

/** "Checks failed — trying again (2/3)": what sent the agent back, and the attempt that starts. */
export function reworkLabel(
  rework: Pick<ReworkContent, "attempt" | "maxAttempts" | "findings">,
): string {
  const sources = new Set(rework.findings.map((f) => f.source));
  const what =
    sources.size === 1 && sources.has("verifier")
      ? "The review found issues"
      : sources.size === 1 && sources.has("ci")
        ? "CI failed"
        : "Checks failed";
  return `${what} — trying again (${rework.attempt}/${rework.maxAttempts})`;
}
