import {
  ACTIVITY,
  activityPartName,
  PURPOSE_PART,
  parsePurpose,
  type TextPurpose,
} from "@/features/chat/lib/agui/vymalo";
import { truncate } from "./findings";
import { drawsStep } from "./steps";

/*
 * Which of an agent's words in a turn are its answer, and which are working text (ADR 0031, plan
 * 10 S6). The chat's column keeps one answer per turn; everything else the agent said while it
 * worked is a note among the steps in the side panel: not collapsed in the chat, not there.
 *
 * What the log says wins. The orchestrator marks the text of a stated stream by the status it came
 * on (`purpose`), and `ThreadAgent` puts that in a marker part right before the text. Text with no
 * mark (a plain A2A agent, the status words, a log written before the mark) is read by the rule of
 * the ADR's point 5, here and nowhere else:
 *
 * - in a turn that is over, the last unmarked text is the answer, and the text before it is working
 *   (when a text is marked `answer`, that is the answer and an unmarked one is working);
 * - in a turn that runs, an unmarked text shows as a draft of the answer until a step starts after
 *   it, and then it folds into the steps. (An artifact, a check or a CI report after the words is
 *   not a step of the agent: the words are still the last thing it said.)
 *
 * Pure: the runtime's parts in, a role per text part out.
 */

type PartLike = { type: string; name?: string; data?: unknown; text?: string };

export type TextRole = "answer" | "working";

const STEP_PART = activityPartName(ACTIVITY.step);
const STATUS_PART = activityPartName(ACTIVITY.status);

/** A step of the agent's own work: a tool, a command, a sub-agent, a status that says what it does. */
const startsStep = (part: PartLike): boolean =>
  part.type === "data" && (part.name === STEP_PART || part.name === STATUS_PART) && drawsStep(part);

/** What the part before a text part says its words are for: the marker `ThreadAgent` puts there. */
export function purposeBefore(
  content: readonly PartLike[],
  index: number,
): TextPurpose | undefined {
  const marker = content[index - 1];
  if (marker?.type !== "data" || marker.name !== PURPOSE_PART) return undefined;
  return parsePurpose((marker.data as { purpose?: unknown } | undefined)?.purpose);
}

/**
 * The role of each text part with words in it, by its index in `content`. A text part that is
 * blank has no role (it draws nothing either way). `running` is whether the turn is still going.
 */
export function textRoles(content: readonly PartLike[], running: boolean): Map<number, TextRole> {
  const texts: { index: number; purpose: TextPurpose | undefined }[] = [];
  content.forEach((part, index) => {
    if (part.type === "text" && part.text?.trim()) {
      texts.push({ index, purpose: purposeBefore(content, index) });
    }
  });
  const roles = new Map<number, TextRole>();
  if (running) {
    for (const { index, purpose } of texts) {
      const folded = content.slice(index + 1).some(startsStep);
      roles.set(index, purpose ?? (folded ? "working" : "answer"));
    }
    return roles;
  }
  const marked = texts.some((t) => t.purpose === "answer");
  const lastUnmarked = texts.findLast((t) => t.purpose === undefined)?.index;
  for (const { index, purpose } of texts) {
    roles.set(index, purpose ?? (!marked && index === lastUnmarked ? "answer" : "working"));
  }
  return roles;
}

/** The longest ticker the page builds; the line cuts it again to its width. */
const TICKER_MAX = 160;

/**
 * The last line of working text as one quiet line: markdown marks the eye would skip are dropped
 * (code ticks, emphasis, a link keeps its words), whitespace is collapsed, and it is cut. Empty
 * when the text says nothing.
 */
export function tickerLine(text: string): string {
  const line =
    text
      .split("\n")
      .map((l) => l.trim())
      .findLast((l) => l !== "") ?? "";
  const plain = line
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[`*]+/g, "")
    .replace(/^#+\s+/, "")
    .replace(/^[-+]\s+/, "")
    .replace(/\s+/g, " ")
    .trim();
  return truncate(plain, TICKER_MAX).text;
}
