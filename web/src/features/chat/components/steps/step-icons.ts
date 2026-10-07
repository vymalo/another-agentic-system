import {
  ArrowRightLeftIcon,
  BotIcon,
  BotMessageSquareIcon,
  BrainIcon,
  CloudDownloadIcon,
  FileIcon,
  FileTextIcon,
  FlaskConicalIcon,
  GitCommitHorizontalIcon,
  GlobeIcon,
  type LucideIcon,
  MessageSquareTextIcon,
  PencilIcon,
  SearchIcon,
  SquareTerminalIcon,
  TerminalIcon,
  Trash2Icon,
  WrenchIcon,
} from "lucide-react";
import type { StepIcon } from "@/features/chat/lib/agui/vymalo";
import type { StepKind } from "@/features/chat/lib/step-tree";

/** The icon vocabulary of steps/v1 (docs/api/steps-v1.md) as the glyphs the page draws. */
export const STEP_ICON: Record<StepIcon, LucideIcon> = {
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
  // OpenCode's own logo is not bundled (its terms are unverified, ADR 0049): a terminal in a frame
  // says "a terminal agent", and the step's label and its tooltip say OpenCode.
  opencode: SquareTerminalIcon,
};

/**
 * What an icon is called, for the tooltip of the glyph and the words a screen reader is told, where
 * the glyph alone would not say it (`opencode` is a terminal in a frame, not OpenCode's logo).
 */
export const STEP_ICON_TITLE: Partial<Record<StepIcon, string>> = {
  opencode: "OpenCode",
};

/** What a step with no icon of its own shows: its kind's. */
export const KIND_ICON: Record<"ask" | "subagent" | "tool" | "command" | "message", LucideIcon> = {
  ask: BotMessageSquareIcon,
  subagent: BotIcon,
  tool: WrenchIcon,
  command: TerminalIcon,
  message: MessageSquareTextIcon,
};

export function iconOf(node: {
  kind: StepKind;
  icon?: StepIcon | undefined;
}): LucideIcon | undefined {
  if (node.icon) return STEP_ICON[node.icon];
  return node.kind === "ask" ||
    node.kind === "subagent" ||
    node.kind === "tool" ||
    node.kind === "command" ||
    node.kind === "message"
    ? KIND_ICON[node.kind]
    : undefined;
}
