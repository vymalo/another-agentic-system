import {
  ACTIVITY,
  ACTOR_PART,
  type ArtifactContent,
  activityPartName,
  type CheckContent,
  PURPOSE_PART,
  parseAnswers,
  parseArtifact,
  parseAsk,
  parseStatus,
  parseStep,
  type ReworkContent,
} from "@/features/chat/lib/agui/vymalo";
import { detectPullRequest, locatePullRequest } from "./artifact";
import { sourceLabel, truncate } from "./findings";

/*
 * How an agent's turn reads (web/DESIGN.md, "A turn"): its activities are steps, which the side
 * panel lists as a tree and the chat sums up in one line, its words are prose, and a pull request
 * or a file is a card after the words. These are the pure decisions behind that (`step-tree.ts`
 * builds the tree); the components are in components/steps and components/cards.
 */

/** The data parts that are steps of the tree (or nothing at all, like the actor marker). */
const STEP_PARTS = new Set(
  [
    ACTIVITY.status,
    ACTIVITY.artifact,
    ACTIVITY.check,
    ACTIVITY.ci,
    ACTIVITY.rework,
    ACTIVITY.action,
    ACTIVITY.step,
    ACTIVITY.ask,
    ACTIVITY.job,
  ].map(activityPartName),
);

/** What a part is, read from the `{type, name, data}` the runtime keeps. */
type PartLike = { type: string; name?: string; data?: unknown; text?: string };

/**
 * An action that answers a Choices: the person's answer (an "Your answers" bubble above the
 * agent's turn), not a step.
 */
export function isAnswerPart(part: PartLike): boolean {
  return (
    part.type === "data" &&
    part.name === activityPartName(ACTIVITY.action) &&
    parseAnswers(part.data) !== null
  );
}

/** The markers `ThreadAgent` puts in the transcript: who ran, and what the next text is for. */
const isMarker = (part: PartLike): boolean =>
  part.name === ACTOR_PART || part.name === PURPOSE_PART;

/**
 * Whether a part belongs in the step list. A failed status is not a step: it is an error in the
 * flow of the turn. The markers go with the steps so that they never split a list.
 */
export function isStepPart(part: PartLike): boolean {
  if (part.type !== "data" || !part.name) return false;
  if (isMarker(part)) return true;
  if (!STEP_PARTS.has(part.name)) return false;
  if (isAnswerPart(part)) return false;
  if (part.name === activityPartName(ACTIVITY.status)) {
    return parseStatus(part.data)?.status !== "failed";
  }
  return true;
}

/**
 * Whether a step part draws a line. The statuses that come with the agent's words (`completed`,
 * `input_required`) say nothing the words and the thread's state do not; the markers and the job
 * marker draw nothing.
 */
export function drawsStep(part: PartLike): boolean {
  if (!isStepPart(part) || isMarker(part)) return false;
  if (part.name === activityPartName(ACTIVITY.job)) return false;
  if (part.name === activityPartName(ACTIVITY.status)) {
    const status = parseStatus(part.data)?.status;
    return status !== "completed" && status !== "input_required" && status !== undefined;
  }
  if (part.name === activityPartName(ACTIVITY.artifact)) return parseArtifact(part.data) !== null;
  if (part.name === activityPartName(ACTIVITY.step)) return parseStep(part.data) !== null;
  if (part.name === activityPartName(ACTIVITY.ask)) return parseAsk(part.data) !== null;
  return true;
}

/**
 * Whether a part draws anything in an agent's turn: words, a step, a failed status, an error, a
 * surface or a card. A message none of whose parts do is no turn at all.
 */
export function drawsPart(part: PartLike): boolean {
  if (part.type === "text") return Boolean(part.text?.trim());
  // what the model thought before it answered (ADR 0044): a closed block above the words
  if (part.type === "reasoning") return Boolean(part.text?.trim());
  if (part.type !== "data" || !part.name) return false;
  if (isStepPart(part)) {
    if (drawsStep(part)) return true;
    const artifact =
      part.name === activityPartName(ACTIVITY.artifact) ? parseArtifact(part.data) : null;
    return artifact !== null && isCardArtifact(artifact);
  }
  if (part.name === activityPartName(ACTIVITY.status)) {
    return parseStatus(part.data)?.status === "failed";
  }
  return (
    part.name === activityPartName(ACTIVITY.error) ||
    part.name === activityPartName(ACTIVITY.surface)
  );
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
  /**
   * Where the link goes, read from it: `acme/demo#12`, `group/project!3`, with the host in front
   * off github.com and gitlab.com (`evil.example/acme/demo#9`), or the bare host.
   */
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
 * checked to be https), or a file whose link is a GitHub pull or GitLab merge request URL. What
 * the card says about where it is (the label, the repository, the number) comes from the URL: an
 * agent's `repository` or `number` that disagrees with it is ignored, and a URL that names
 * neither is labelled by its host.
 */
export function pullRequestOf(artifact: ArtifactContent): PullRequestView | undefined {
  if (artifact.kind === "pull_request" && artifact.url) {
    const where = locatePullRequest(artifact.url);
    if (!where) return undefined;
    const title = titleOf(artifact.text);
    return {
      href: artifact.url,
      label: where.label ?? where.host,
      ...(where.number !== undefined ? { number: where.number } : {}),
      ...(where.repository ? { repository: shortRepository(where.repository) } : {}),
      ...(artifact.branch ? { branch: artifact.branch } : {}),
      ...(title ? { title } : {}),
    };
  }
  const detected = artifact.kind === "file" ? detectPullRequest(artifact.uri) : undefined;
  if (!detected) return undefined;
  const where = locatePullRequest(detected.href);
  return {
    href: detected.href,
    label: detected.label,
    ...(where?.number !== undefined ? { number: where.number } : {}),
    ...(where?.repository ? { repository: shortRepository(where.repository) } : {}),
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

/** A card after the agent's words: a pull request, or a file that is not one. */
export type TurnCard = { pr: PullRequestView } | { file: ArtifactContent };

/**
 * The cards of a turn from its artifacts, in order. A pull request (or a linked file) reported
 * again, as a rework attempt does, is one card: the last report of its link stands, where it
 * came. A file the store kept is one card per hash. Files without a link are all kept.
 */
export function turnCards(artifacts: readonly ArtifactContent[]): TurnCard[] {
  const cards = artifacts.filter(isCardArtifact).map((a): { card: TurnCard; href?: string } => {
    const pr = pullRequestOf(a);
    if (pr) return { card: { pr }, href: pr.href };
    // a kept file is told by its hash, a linked one by its link
    const key = a.href ?? a.uri;
    return { card: { file: a }, ...(key ? { href: key } : {}) };
  });
  return cards
    .filter((c, i) => c.href === undefined || !cards.slice(i + 1).some((d) => d.href === c.href))
    .map((c) => c.card);
}
