import { fromMarkdown } from "mdast-util-from-markdown";
import { gfmFromMarkdown } from "mdast-util-gfm";
import { gfm } from "micromark-extension-gfm";
import { safeHttpUrl } from "@/features/chat/lib/a2ui/url";
import {
  ACTIVITY,
  activityPartName,
  parseArtifact,
  parseCi,
} from "@/features/chat/lib/agui/vymalo";
import { safeLinkHref } from "@/features/chat/lib/artifact";
import { conclusionLabel, providerLabel } from "@/features/chat/lib/ci";
import { downloadHref, fileTitle, formatSize, keptFileOf } from "@/features/chat/lib/files";
import { truncate } from "@/features/chat/lib/findings";
import { drawsPart, pullRequestOf, shortRepository } from "@/features/chat/lib/steps";

/*
 * What the agents shared, for the panel's Sources tab: derived in the browser from what the runtime
 * already holds (the typed artifacts and CI reports of the projection, and the links in the agents'
 * words). Nothing is new on the wire. Everything here is agent output, so nothing becomes a link
 * unless it is an absolute http(s) URL (`safeHttpUrl`), and what is shown is text.
 */

/** What a part looks like in the runtime's messages. */
type PartLike = { type: string; name?: string; text?: string; data?: unknown };
export type SourceMessage = { id: string; role: string; content: readonly PartLike[] };

export type SourceKind = "pull_request" | "branch" | "ci" | "file" | "link";
export type SourceGroupId = "code" | "checks" | "files" | "links";

/** One agent turn, as the chat numbers them: the nth assistant message that draws something. */
export type SourceTurn = { id: string; number: number };

export type Source = {
  /** The normalised URL; a branch without one is `branch:<repository>#<branch>`. */
  key: string;
  kind: SourceKind;
  /** What to call it: text from the agent, never markup. */
  title: string;
  /** A muted second part after the kind: a host, a provider, a type. */
  detail?: string;
  /**
   * Only an absolute http(s) URL, or the API's route of a file the store kept; absent for what
   * cannot be opened (a branch).
   */
  href?: string;
  /** A kept file: the same file as an attachment (`?download=1`). */
  downloadHref?: string;
  /** CI: how the report counts. */
  passed?: boolean;
  /** Every turn that cited it, in order. */
  turns: SourceTurn[];
};

export type SourceGroup = { id: SourceGroupId; label: string; items: Source[] };

const GROUPS: readonly { id: SourceGroupId; label: string }[] = [
  { id: "code", label: "Pull requests & branches" },
  { id: "checks", label: "Checks" },
  { id: "files", label: "Files" },
  { id: "links", label: "Links" },
];

const GROUP_OF: Record<SourceKind, SourceGroupId> = {
  pull_request: "code",
  branch: "code",
  ci: "checks",
  file: "files",
  link: "links",
};

export const KIND_LABEL: Record<SourceKind, string> = {
  pull_request: "Pull request",
  branch: "Branch",
  ci: "CI",
  file: "File",
  link: "Link",
};

/** More links than this in one text are not looked at: a text is not a link farm. */
const MAX_LINKS_PER_TEXT = 100;
const MAX_TITLE = 120;

/**
 * The URL without what makes two spellings of one link differ: the scheme and host in lower case
 * (the URL parser does that), no default port, no trailing slash on the path. The fragment stays:
 * `#section` is somewhere else in the page.
 */
export function normaliseUrl(href: string): string {
  const url = new URL(href);
  const path = url.pathname.replace(/\/+$/, "") || "/";
  return `${url.protocol}//${url.host}${path}${url.search}${url.hash}`;
}

const hostOf = (href: string): string => new URL(href).host;

/** `docs.rs/axum/latest`: what a link with no words of its own is called. */
function shown(href: string): string {
  const url = new URL(href);
  const path = url.pathname.replace(/\/+$/, "");
  return truncate(`${url.host}${path}`, MAX_TITLE).text;
}

// ---- links in the agents' words --------------------------------------------------------------

type MdNode = {
  type: string;
  url?: string;
  value?: string;
  alt?: string | null;
  identifier?: string;
  children?: MdNode[];
};

/** The plain text under a node: text, inline code and image descriptions; spaces collapsed. */
function textOf(node: MdNode): string {
  const own =
    node.type === "text" || node.type === "inlineCode"
      ? (node.value ?? "")
      : node.type === "image"
        ? (node.alt ?? "")
        : "";
  return own + (node.children ?? []).map(textOf).join("");
}

export type TextLink = { href: string; text: string };

/**
 * The links of a markdown text: `[text](url)`, `[text][ref]` with its definition, `<url>` and the
 * autolinks GFM makes of a bare URL. Parsed, so a URL inside a code span or a code block is no
 * link (it is not one in the chat either). Only http(s); in order; none twice in one text.
 */
export function linksIn(markdown: string): TextLink[] {
  if (!markdown.includes("http") && !markdown.includes("HTTP")) return [];
  const tree = fromMarkdown(markdown, {
    extensions: [gfm()],
    mdastExtensions: [gfmFromMarkdown()],
  }) as unknown as MdNode;

  const definitions = new Map<string, string>();
  const found: { url: string; text: string }[] = [];
  const walk = (node: MdNode) => {
    if (node.type === "definition" && node.identifier && node.url !== undefined) {
      if (!definitions.has(node.identifier)) definitions.set(node.identifier, node.url);
    }
    if (node.type === "link" && node.url !== undefined) {
      found.push({ url: node.url, text: textOf(node) });
    } else if (node.type === "linkReference" && node.identifier) {
      found.push({ url: `\u0000ref:${node.identifier}`, text: textOf(node) });
    }
    // a link inside a link cannot be (markdown does not nest them); the children of any other node
    for (const child of node.children ?? []) walk(child);
  };
  walk(tree);

  const out: TextLink[] = [];
  const seen = new Set<string>();
  for (const { url, text } of found) {
    const raw = url.startsWith("\u0000ref:") ? definitions.get(url.slice(5)) : url;
    const href = safeHttpUrl(raw);
    if (!href) continue;
    const key = normaliseUrl(href);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push({ href, text: text.replace(/\s+/g, " ").trim() });
    if (out.length >= MAX_LINKS_PER_TEXT) break;
  }
  return out;
}

// ---- the sources of one message ---------------------------------------------------------------

type Candidate = Omit<Source, "turns">;

const ARTIFACT_PART = activityPartName(ACTIVITY.artifact);
const CI_PART = activityPartName(ACTIVITY.ci);

function fromArtifact(data: unknown): Candidate | null {
  const artifact = parseArtifact(data);
  if (!artifact) return null;
  const pr = pullRequestOf(artifact);
  if (pr) {
    return {
      key: normaliseUrl(pr.href),
      kind: "pull_request",
      title: pr.title ? `${pr.label} — ${pr.title}` : pr.label,
      detail: hostOf(pr.href),
      href: pr.href,
    };
  }
  if (artifact.kind === "branch") {
    if (!artifact.branch || !artifact.repository) return null;
    // the projection's `repository` is `host/owner/name`: a link only when it really is a URL
    const href = artifact.repository.startsWith("https://")
      ? safeHttpUrl(artifact.repository)
      : undefined;
    return {
      key: `branch:${artifact.repository}#${artifact.branch}`,
      kind: "branch",
      title: `${artifact.branch} · ${shortRepository(artifact.repository)}`,
      ...(href ? { href } : {}),
    };
  }
  if (artifact.kind === "file") {
    // a file the store kept: listed by its hash, opened and downloaded from the API's route
    const kept = keptFileOf(artifact);
    if (kept) {
      return {
        key: `file:${kept.sha256}`,
        kind: "file",
        title: truncate(fileTitle(kept), MAX_TITLE).text,
        detail: [formatSize(kept.size), kept.mimeType].filter(Boolean).join(" · "),
        href: kept.href,
        downloadHref: downloadHref(kept),
      };
    }
    const href = safeLinkHref(artifact.uri);
    if (!href) return null;
    return {
      key: normaliseUrl(href),
      kind: "file",
      title: truncate(artifact.name, MAX_TITLE).text,
      detail: artifact.mimeType ?? hostOf(href),
      href,
    };
  }
  return null;
}

function fromCi(data: unknown): Candidate | null {
  const ci = parseCi(data);
  const href = ci?.url ? safeHttpUrl(ci.url) : undefined;
  if (!ci || !href) return null;
  const outcome = ci.passed ? "passed" : conclusionLabel(ci.conclusion).toLowerCase();
  return {
    key: normaliseUrl(href),
    kind: "ci",
    title: `CI: ${truncate(ci.name, 80).text} — ${outcome}`,
    detail: providerLabel(ci.provider),
    href,
    passed: ci.passed,
  };
}

/**
 * A text's links are the same each time it is asked for, and the runtime hands over the whole
 * transcript on every streamed word: parse each distinct text once (a streaming text is a new
 * string each time, an earlier turn's is the same one). Bounded, so a long thread cannot grow it.
 */
const MEMO_LIMIT = 400;
const textMemo = new Map<string, readonly Candidate[]>();

function fromText(text: string): readonly Candidate[] {
  const known = textMemo.get(text);
  if (known) return known;
  const links = linksIn(text).map(
    ({ href, text: words }): Candidate => ({
      key: normaliseUrl(href),
      kind: "link",
      // a link with no words of its own (an autolink: its text is the URL) is called by host and path
      title: words && !/^https?:\/\//i.test(words) ? truncate(words, MAX_TITLE).text : shown(href),
      detail: hostOf(href),
      href,
    }),
  );
  if (textMemo.size >= MEMO_LIMIT) textMemo.clear();
  textMemo.set(text, links);
  return links;
}

function candidatesOf(message: SourceMessage): readonly Candidate[] {
  const out: Candidate[] = [];
  for (const part of message.content) {
    if (part.type === "text" && part.text) out.push(...fromText(part.text));
    else if (part.type === "data" && part.name === ARTIFACT_PART) {
      const found = fromArtifact(part.data);
      if (found) out.push(found);
    } else if (part.type === "data" && part.name === CI_PART) {
      const found = fromCi(part.data);
      if (found) out.push(found);
    }
  }
  return out;
}

// ---- the sources of a thread ------------------------------------------------------------------

/** The agent turns, numbered as the chat shows them (a message that draws nothing is no turn). */
export function agentTurns(messages: readonly SourceMessage[]): SourceTurn[] {
  const turns: SourceTurn[] = [];
  for (const message of messages) {
    if (message.role === "assistant" && message.content.some(drawsPart)) {
      turns.push({ id: message.id, number: turns.length + 1 });
    }
  }
  return turns;
}

/**
 * Everything the agents shared in a thread, in groups (pull requests and branches, checks, files,
 * links), in the order it first appeared. One URL is one item, however often it was cited: it keeps
 * every turn that did. A typed source (pull request, CI report, file) is the better description of
 * a URL than the same URL in someone's words, so it takes the place of the link, in its position.
 */
export function collectSources(messages: readonly SourceMessage[]): SourceGroup[] {
  const turnOf = new Map(agentTurns(messages).map((t) => [t.id, t]));
  const found = new Map<string, Source>();
  for (const message of messages) {
    const turn = turnOf.get(message.id);
    if (!turn) continue;
    for (const candidate of candidatesOf(message)) {
      const known = found.get(candidate.key);
      if (!known) {
        found.set(candidate.key, { ...candidate, turns: [turn] });
        continue;
      }
      const base = known.kind === "link" && candidate.kind !== "link" ? candidate : known;
      const turns = known.turns.some((t) => t.id === turn.id)
        ? known.turns
        : [...known.turns, turn];
      found.set(candidate.key, { ...base, turns });
    }
  }
  const items = [...found.values()];
  return GROUPS.flatMap(({ id, label }) => {
    const inGroup = items.filter((s) => GROUP_OF[s.kind] === id);
    return inGroup.length > 0 ? [{ id, label, items: inGroup }] : [];
  });
}

export const countSources = (groups: readonly SourceGroup[]): number =>
  groups.reduce((n, g) => n + g.items.length, 0);
