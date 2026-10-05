import { EventType } from "@ag-ui/client";
import type { ApiActor } from "@/lib/api/types";
import { ACTOR_KEY, PURPOSE_KEY, parseActor, parsePurpose, type TextPurpose } from "./vymalo";

/**
 * Live text, the web's side (docs/api/agui.md "Live text", ADR 0027): the words of a reply that is
 * still being written, shown while they are written, and never stored.
 *
 * The orchestrator says them as `TEXT_MESSAGE_*` frames marked `metadata["vymalo.live"]`, which are
 * **not** the log: they have no `id:` (never a resume point), and the log says the reply once, final,
 * under the same message id. So the web keeps them out of the runtime, whose transcript is the log
 * and which would refuse a message id the log lacks (`translate`, 422): they are **drafts**, held
 * here, drawn after the turn's parts, and handed to the runtime only as the one message the log
 * says, when the log says it.
 *
 * | Frame | What it does |
 * |---|---|
 * | `START` `{}` | opens a draft: the id, the agent's name, the invocation, the actor |
 * | `CONTENT` `{offset}` | `text = text.slice(0, offset) + delta`, when that grows it; an offset beyond what is held is a gap and says nothing |
 * | `END` `{abandoned}` | the generation was given up: the draft goes |
 * | `CONTENT` `{offset, final}` + `END` `{final}` | the log's message for the id, in the group of its event: the draft's text up to `offset` plus the rest, as the plain `START`/`CONTENT`/`END` the runtime reads |
 * | `END` `{final, purpose: "working"}` | the same, and the words were not the answer (ADR 0031): the draft stops being drawn in the conversation and the runtime's `START` says `vymalo.purpose: working`, so the turn files the text with its steps. An `END` that says only `final` is an answer (or unmarked text): the draft stays |
 *
 * `offset` is in UTF-16 code units, the unit of a JS string, so a client slices with it as it is.
 * Everything here is pure: `ThreadAgent` keeps the list and calls these on each frame.
 *
 * **Reasoning is a second kind of draft** (ADR 0044, "Reasoning" of docs/api/agui.md), in AG-UI's reasoning
 * events, with the same rules: the live `REASONING_START` opens a draft with `kind: "reasoning"` (the
 * `REASONING_MESSAGE_START` that follows says nothing more), `REASONING_MESSAGE_CONTENT {offset}` grows it, the two ends
 * with `abandoned` remove it, and the log's reasoning, in the group of its event as `REASONING_MESSAGE_CONTENT
 * {offset, final}`, `REASONING_MESSAGE_END {final}` and `REASONING_END {final}` (the two `START`s were dropped by the overlay),
 * becomes the plain five events the runtime reads, which it turns into a reasoning part of the turn.
 */

/** `metadata` key of every live frame. */
export const LIVE_KEY = "vymalo.live";

/** The most text one draft holds: the contract's bound (256 KiB of UTF-8) is at most this many units. */
export const MAX_DRAFT_UNITS = 262_144;
/** How many drafts are held: the overlay opens one at a time, so this is a bound on a faulty stream. */
export const MAX_DRAFTS = 8;

/** A reply being written, or the reasoning that came before it (`kind: "reasoning"`). */
export type Draft = {
  /** `"reasoning"` for what the model thinks before it answers (ADR 0044); a reply when absent. */
  kind?: "reasoning";
  /** The message id: the id of the log's message that completes it. */
  id: string;
  /** What was said so far. */
  text: string;
  /** The agent that writes it. */
  name?: string;
  /** The invocation (subagent run) it belongs to. */
  subagentRunId?: string;
  actor?: ApiActor;
  /**
   * The log's message arrived and went to the runtime: this is its whole text. The draft stays
   * until the next group (the transcript has the message by then) and draws nothing once the
   * transcript shows these words, so the text never goes missing nor appears twice.
   */
  final?: string;
  /**
   * What the log's message said its words were for, when its `END` said so (ADR 0031): only
   * `"working"` is ever said there. A working draft draws nothing in the conversation: its text is
   * a note of the turn's steps.
   */
  purpose?: TextPurpose;
};

/** A frame as `ThreadAgent` reads it. */
export type LiveEvent = { type: string } & Record<string, unknown>;

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** What `metadata["vymalo.live"]` of a text frame says. */
export type LiveMark = {
  /** UTF-16 code units said before the delta; undefined when absent or not a count. */
  offset: number | undefined;
  /** The log's own message: the frame is part of a group, with the resume point on its `END`. */
  final: boolean;
  /** The reply was given up (on an `END`). */
  abandoned: boolean;
  /** What the words were for, said on the `END` of the log's own message (ADR 0031). */
  purpose: TextPurpose | undefined;
};

/** The frames that can carry a live mark: the words of a reply and the reasoning before it. */
const LIVE_TYPES: ReadonlySet<string> = new Set([
  EventType.TEXT_MESSAGE_START,
  EventType.TEXT_MESSAGE_CONTENT,
  EventType.TEXT_MESSAGE_END,
  EventType.REASONING_START,
  EventType.REASONING_MESSAGE_START,
  EventType.REASONING_MESSAGE_CONTENT,
  EventType.REASONING_MESSAGE_END,
  EventType.REASONING_END,
]);

/** The live mark of a text-message or reasoning frame, or null for any other frame. */
export function liveMark(event: LiveEvent): LiveMark | null {
  if (!LIVE_TYPES.has(event.type)) return null;
  const live = isRecord(event.metadata) ? event.metadata[LIVE_KEY] : undefined;
  if (!isRecord(live)) return null;
  const offset = live.offset;
  return {
    offset:
      typeof offset === "number" && Number.isSafeInteger(offset) && offset >= 0
        ? offset
        : undefined,
    final: live.final === true,
    abandoned: live.abandoned === true,
    purpose: parsePurpose(live.purpose),
  };
}

/**
 * A live frame that is not the log's own: handled when it arrives, never grouped, and never passed
 * to the runtime. (The frames of the log's final message carry the mark too, `final`, and travel
 * in their group.)
 */
export const isLiveNow = (event: LiveEvent): boolean => {
  const mark = liveMark(event);
  return mark !== null && !mark.final;
};

/** The drafts after one live frame (`isLiveNow`); the same array when it changes nothing. */
export function applyLive(drafts: readonly Draft[], event: LiveEvent): readonly Draft[] {
  const mark = liveMark(event);
  const id = str(event.messageId);
  if (!mark || mark.final || !id) return drafts;
  switch (event.type) {
    case EventType.TEXT_MESSAGE_START:
    case EventType.REASONING_START: {
      const reasoning = event.type === EventType.REASONING_START;
      const name = str(event.name);
      const sub = str(event.subagentRunId);
      const meta = isRecord(event.metadata) ? event.metadata : undefined;
      const actor = parseActor(meta?.[ACTOR_KEY]);
      const draft: Draft = {
        id,
        text: "",
        ...(reasoning ? { kind: "reasoning" as const } : {}),
        ...(name ? { name } : {}),
        ...(sub ? { subagentRunId: sub } : {}),
        ...(actor ? { actor } : {}),
      };
      return [...drafts.filter((d) => d.id !== id), draft].slice(-MAX_DRAFTS);
    }
    case EventType.TEXT_MESSAGE_CONTENT:
    case EventType.REASONING_MESSAGE_CONTENT: {
      const at = drafts.findIndex((d) => d.id === id);
      const draft = drafts[at];
      if (!draft || mark.offset === undefined) return drafts;
      // a gap: the sender says the text again from the start within a second
      if (mark.offset > draft.text.length) return drafts;
      const text = draft.text.slice(0, mark.offset) + (str(event.delta) ?? "");
      // a piece that repeats what is held says nothing; one past the bound is not shown
      if (text.length <= draft.text.length || text.length > MAX_DRAFT_UNITS) return drafts;
      return drafts.map((d, i) => (i === at ? { ...d, text } : d));
    }
    case EventType.TEXT_MESSAGE_END:
    case EventType.REASONING_MESSAGE_END:
    case EventType.REASONING_END:
      return mark.abandoned && drafts.some((d) => d.id === id)
        ? drafts.filter((d) => d.id !== id)
        : drafts;
    default:
      // the live REASONING_MESSAGE_START opened nothing: the REASONING_START did
      return drafts;
  }
}

/** A group of frames, closed by its `id:`, with the drafts the runtime may not see taken out. */
export type Resolved = {
  /** What the runtime gets: the log's message complete (START, CONTENT, END) where a draft was open. */
  events: LiveEvent[];
  drafts: readonly Draft[];
};

/**
 * Turns the log's final message frames in a group into the plain triad the runtime reads, using the
 * draft the group continues: `CONTENT{offset, final}` becomes `START` + `CONTENT(text up to offset +
 * delta)`, its `END` loses the mark. A frame that is not the log's final message passes as it is.
 *
 * Null when the group cannot be told whole: it continues text this connection never held (a draft
 * missing, or shorter than `offset`), or ends a message it never started. The caller drops the
 * group and reconnects at its last resume point; the new connection says the message plainly,
 * because its overlay starts empty. `offset: 0` needs no draft: the frame is the whole text (a
 * final that replaced what was said).
 */
export function resolveGroup(drafts: readonly Draft[], events: LiveEvent[]): Resolved | null {
  if (!events.some((e) => liveMark(e)?.final)) return { events, drafts };
  const whole = new Map<string, string>();
  const out: LiveEvent[] = [];
  // what the words were for is said on the `END`, after the `CONTENT` that the `START` is made at
  const purposes = new Map<string, TextPurpose>();
  for (const event of events) {
    const mark = liveMark(event);
    const id = str(event.messageId);
    if (mark?.final && mark.purpose && id && event.type === EventType.TEXT_MESSAGE_END) {
      purposes.set(id, mark.purpose);
    }
  }
  for (const event of events) {
    const mark = liveMark(event);
    const id = str(event.messageId);
    if (!mark?.final || !id) {
      out.push(event);
      continue;
    }
    const draft = drafts.find((d) => d.id === id);
    const { metadata: _live, ...bare } = event;
    if (event.type === EventType.TEXT_MESSAGE_CONTENT) {
      if (mark.offset === undefined) return null;
      if (mark.offset > 0 && (!draft || draft.text.length < mark.offset)) return null;
      const text = (draft ? draft.text.slice(0, mark.offset) : "") + (str(event.delta) ?? "");
      whole.set(id, text);
      const name = draft?.name;
      const sub = draft?.subagentRunId ?? str(event.subagentRunId);
      const purpose = purposes.get(id);
      out.push({
        type: EventType.TEXT_MESSAGE_START,
        messageId: id,
        role: "assistant",
        ...(name ? { name } : {}),
        ...(sub ? { subagentRunId: sub } : {}),
        ...(draft?.actor || purpose
          ? {
              metadata: {
                ...(draft?.actor ? { [ACTOR_KEY]: draft.actor } : {}),
                ...(purpose ? { [PURPOSE_KEY]: purpose } : {}),
              },
            }
          : {}),
      });
      out.push({ ...bare, delta: text });
    } else if (event.type === EventType.REASONING_MESSAGE_CONTENT) {
      // the log's reasoning (ADR 0044): the overlay dropped its two STARTs when it continued a live one, so they are
      // made here, from the draft, and the runtime reads the five events of a reasoning span as the log wrote them
      if (mark.offset === undefined) return null;
      if (mark.offset > 0 && (!draft || draft.text.length < mark.offset)) return null;
      const text = (draft ? draft.text.slice(0, mark.offset) : "") + (str(event.delta) ?? "");
      whole.set(id, text);
      const sub = draft?.subagentRunId ?? str(event.subagentRunId);
      const attribution = sub ? { subagentRunId: sub } : {};
      out.push({ type: EventType.REASONING_START, messageId: id, ...attribution });
      out.push({
        type: EventType.REASONING_MESSAGE_START,
        messageId: id,
        role: "reasoning",
        ...attribution,
      });
      out.push({ ...bare, delta: text });
    } else if (
      event.type === EventType.TEXT_MESSAGE_END ||
      event.type === EventType.REASONING_MESSAGE_END ||
      event.type === EventType.REASONING_END
    ) {
      if (!whole.has(id)) return null;
      out.push(bare);
    } else {
      return null; // a START with the final mark: the overlay never says one
    }
  }
  const next = drafts.map((d) => {
    const text = whole.get(d.id);
    if (text === undefined) return d;
    const purpose = purposes.get(d.id);
    return { ...d, final: text, ...(purpose ? { purpose } : {}) };
  });
  return { events: out, drafts: next };
}

/** The drafts that are not yet merged: what stays when the log's message has reached the transcript. */
export const pending = (drafts: readonly Draft[]): readonly Draft[] =>
  drafts.some((d) => d.final !== undefined) ? drafts.filter((d) => d.final === undefined) : drafts;

/**
 * The drafts a turn draws, given the text parts its transcript holds: a merged draft says the
 * log's words until the transcript has them, and then nothing, so the swap is one render, and the
 * words never go missing nor show twice. An empty draft draws nothing, and neither does one whose
 * `END` said it was working text (ADR 0031): that is a note of the steps, never a reply.
 */
export function drawnDrafts(
  drafts: readonly Draft[],
  texts: readonly string[],
): { id: string; text: string; name?: string }[] {
  const out: { id: string; text: string; name?: string }[] = [];
  for (const d of drafts) {
    // reasoning is drawn by its own block (`drawnReasoning`), never as a reply
    if (d.kind === "reasoning") continue;
    // the words turned out to be working text: a note of the turn's steps, not a reply
    if (d.purpose === "working") continue;
    const text = d.final ?? d.text;
    if (d.final !== undefined && texts.some((t) => t.includes(text))) continue;
    if (text.trim() === "") continue;
    out.push({ id: d.id, text, ...(d.name ? { name: d.name } : {}) });
  }
  return out;
}

/**
 * The reasoning drafts a turn draws, given the reasoning parts its transcript holds: the same swap as
 * [`drawnDrafts`]: a merged draft says the log's words until the transcript has them, and then nothing.
 * An empty draft draws nothing.
 */
export function drawnReasoning(
  drafts: readonly Draft[],
  texts: readonly string[],
): { id: string; text: string; done: boolean }[] {
  const out: { id: string; text: string; done: boolean }[] = [];
  for (const d of drafts) {
    if (d.kind !== "reasoning") continue;
    const text = d.final ?? d.text;
    if (d.final !== undefined && texts.some((t) => t.includes(text))) continue;
    if (text.trim() === "") continue;
    out.push({ id: d.id, text, done: d.final !== undefined });
  }
  return out;
}
