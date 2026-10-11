import {
  AppWindowIcon,
  ArrowRightLeftIcon,
  BookOpenIcon,
  BotIcon,
  BotMessageSquareIcon,
  BrainIcon,
  CloudDownloadIcon,
  FileIcon,
  FileTextIcon,
  FlaskConicalIcon,
  GitCommitHorizontalIcon,
  GlobeIcon,
  MessageSquareTextIcon,
  PencilIcon,
  SearchIcon,
  TerminalIcon,
  Trash2Icon,
  WrenchIcon,
} from "lucide-react";
import { GitHubMark } from "@/components/brand/github-mark";
import { OpenCodeMark } from "@/components/brand/opencode-mark";
import type { Glyph } from "@/components/glyph";
import type { StepIcon } from "@/features/chat/lib/agui/vymalo";
import { toolName } from "@/features/chat/lib/step-label";
import type { StepKind } from "@/features/chat/lib/step-tree";

/**
 * The icon vocabulary of steps/v1 (docs/api/steps-v1.md) as the glyphs the page draws: lucide's,
 * but `opencode`, which is OpenCode's own mark (ADR 0049, amended 2026-10-11).
 */
export const STEP_ICON: Record<StepIcon, Glyph> = {
  agent: BotIcon,
  read: FileTextIcon,
  edit: PencilIcon,
  delete: Trash2Icon,
  move: ArrowRightLeftIcon,
  search: SearchIcon,
  execute: TerminalIcon,
  think: BrainIcon,
  fetch: CloudDownloadIcon,
  web: GlobeIcon,
  git: GitCommitHorizontalIcon,
  test: FlaskConicalIcon,
  file: FileIcon,
  tool: WrenchIcon,
  opencode: OpenCodeMark,
};

/**
 * What an icon is called, for the tooltip of the glyph and the words a screen reader is told, where
 * the glyph alone would not say it (a mark has no words).
 */
export const STEP_ICON_TITLE: Partial<Record<StepIcon, string>> = {
  opencode: "OpenCode",
};

/** What a step with no icon of its own shows: its kind's. */
export const KIND_ICON: Record<"ask" | "subagent" | "tool" | "command" | "message", Glyph> = {
  ask: BotMessageSquareIcon,
  subagent: BotIcon,
  tool: WrenchIcon,
  command: TerminalIcon,
  message: MessageSquareTextIcon,
};

/** A server the page knows by the name of its tools (`github__list_commits`): how it is drawn. */
export type KnownServer = {
  /** What a person calls it: the glyph's tooltip. */
  name: string;
  /** Its mark if it has a logo the page bundles, else a glyph that says what it is for. */
  icon: Glyph;
  /** A logo stands for the server, so the step does not name the server again; a glyph does not. */
  logo: boolean;
};

const BROWSER: KnownServer = { name: "Browser", icon: AppWindowIcon, logo: false };

/** By the `server` of `server__tool`, the id the agent's `mcp.json` gives it. */
const KNOWN_SERVERS: Readonly<Record<string, KnownServer>> = {
  github: { name: "GitHub", icon: GitHubMark, logo: true },
  browser: BROWSER,
  obscura: BROWSER,
  websearch: { name: "Web search", icon: SearchIcon, logo: false },
  context7: { name: "Context7", icon: BookOpenIcon, logo: false },
};

/** The known server a tool's label names (`github__list_commits`), if it does. */
export function knownServer(label: string): KnownServer | undefined {
  const server = toolName(label).server?.toLowerCase();
  return server !== undefined && Object.hasOwn(KNOWN_SERVERS, server)
    ? KNOWN_SERVERS[server]
    : undefined;
}

/** What a step draws in its icon slot, and what to call it where the glyph alone would not say. */
export type StepGlyph = { icon: Glyph | undefined; title: string | undefined };

/**
 * The glyph of a step. A server's logo wins over any icon an agent gave its call (the mark says
 * where the call went); the glyph of a server only stands in for the generic one, so an icon an
 * agent chose (`read`, `web`) is kept; then the step's kind's.
 */
export function stepGlyph(node: {
  kind: StepKind;
  icon?: StepIcon | undefined;
  label: string;
}): StepGlyph {
  const server = node.kind === "tool" ? knownServer(node.label) : undefined;
  if (server?.logo) return { icon: server.icon, title: server.name };
  if (server && (node.icon === undefined || node.icon === "tool")) {
    return { icon: server.icon, title: server.name };
  }
  if (node.icon) return { icon: STEP_ICON[node.icon], title: STEP_ICON_TITLE[node.icon] };
  return {
    icon:
      node.kind === "ask" ||
      node.kind === "subagent" ||
      node.kind === "tool" ||
      node.kind === "command" ||
      node.kind === "message"
        ? KIND_ICON[node.kind]
        : undefined,
    title: undefined,
  };
}

export function iconOf(node: {
  kind: StepKind;
  icon?: StepIcon | undefined;
  label?: string;
}): Glyph | undefined {
  return stepGlyph({ ...node, label: node.label ?? "" }).icon;
}
