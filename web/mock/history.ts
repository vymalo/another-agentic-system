/**
 * The mock's copy of the orchestrator's history fold (`orch-agui-projection`, `history.rs`; docs/api/history.md): a finite
 * page of the frames a connect stream writes for a replay, a whole number of settled chains. `history.test.ts` checks the
 * boundaries it finds against the ones the real fold writes in docs/api/examples/history, for the same logs, so this
 * cannot drift from the real one unnoticed.
 *
 * A chain is the events from one that opens a run while none is open to the first event after which none is; a steering
 * message extends it, events with no frame ride along with the chain before, the first chain starts at event 1. A turn is a
 * chain a person's message opens (the first counts as one). A page never holds the chain that is still open.
 */
import type { components } from "../src/lib/api/schema";
import { type Carry, CarryLog, type Prefix } from "./carry";
import { type Frame, Projector, type ThreadInfo } from "./projection";

type Event = components["schemas"]["Event"];

/** The version of the projection the mock says its frames are (`ui.history.projection`). */
export const PROJECTION_VERSION = 1;

/** The most chains kept for a thread whose turns do not bound it. */
const MAX_KEPT_CHAINS = 512;

export type HistoryLimits = { maxTurns: number; maxPageBytes: number };

export const DEFAULT_LIMITS: HistoryLimits = { maxTurns: 100, maxPageBytes: 4 * 1024 * 1024 };

export type Window =
  | { kind: "turns"; before?: number; turns: number }
  | { kind: "since"; before?: number; since: number }
  | { kind: "after"; after: number };

export type Page = {
  start: number;
  end: number;
  head: number;
  earlier: boolean;
  frames: Frame[];
  anchor?: { seq: number; runId: string };
  /** What the log before `start` contributes to the readouts that cover the whole thread; with `earlier`, not for a catch-up. */
  carry?: Carry;
};

type Chain = { start: number; end: number; turn: boolean; frames: Frame[]; prefix: Prefix };

const frameBytes = (frame: Frame): number => JSON.stringify(frame.event).length + 24;
const chainBytes = (chain: Chain): number => chain.frames.reduce((n, f) => n + frameBytes(f), 0);

export class History {
  private readonly projector: Projector;
  private readonly window: Window;
  private readonly chains: Chain[] = [];
  private turns = 0;
  private readonly keepTurns: number;
  private started = 0;
  private ran = false;
  private run: string | null = null;
  private anchor: { seq: number; runId: string } | undefined;
  private capped = false;
  private lastSeq = 0;
  private done = false;
  private readonly head: number;
  private readonly carry = new CarryLog();
  /** Where the pass stood at the start of the chain the page leaves to the stream or to the caller. */
  private tail: Prefix | undefined;

  constructor(
    info: ThreadInfo,
    window: Window,
    private readonly limits: HistoryLimits,
    head: number,
  ) {
    this.head = Math.max(0, head);
    const maxTurns = Math.max(1, limits.maxTurns);
    const inRange = (b: number | undefined) =>
      b !== undefined && b >= 1 && b <= this.head ? b : undefined;
    this.window =
      window.kind === "turns"
        ? { kind: "turns", before: inRange(window.before), turns: clamp(window.turns, 1, maxTurns) }
        : window.kind === "since"
          ? { kind: "since", before: inRange(window.before), since: window.since }
          : { kind: "after", after: clamp(window.after, 0, this.head) };
    this.keepTurns = (this.window.kind === "turns" ? this.window.turns : maxTurns) + 1;
    this.projector = new Projector(info);
  }

  private stopAfter(): number | undefined {
    return this.window.kind === "after" ? undefined : this.window.before;
  }

  /** Folds the next event; true while more are needed. */
  feed(event: Event): boolean {
    if (this.done || event.seq > this.head) {
      this.done = true;
      return false;
    }
    const wasOpen = this.projector.runOpen;
    const frames = this.projector.apply(event);
    this.lastSeq = event.seq;
    const opens = !wasOpen && frames.some((f) => f.event.type === "RUN_STARTED");
    const starts = this.started === 0 || (opens && this.ran);
    this.ran ||= opens;
    this.track(event.seq, frames);
    if (starts && !this.startChain(event)) {
      this.done = true;
      this.capped = true;
      return false;
    }
    // a catch-up has no carry: the caller holds what came before
    if (this.window.kind !== "after") this.carry.observe(frames);
    const back = this.chains.at(-1);
    if (back) {
      back.end = event.seq;
      back.frames.push(...frames);
    }
    const stop = this.stopAfter();
    if (stop !== undefined && event.seq >= stop) {
      this.done = true;
      return false;
    }
    return true;
  }

  private startChain(event: Event): boolean {
    const turn = this.started === 0 || event.kind === "user_message";
    this.started++;
    if (this.window.kind === "after") {
      if (event.seq <= this.window.after) return true;
      if (turn && this.turns >= Math.max(1, this.limits.maxTurns)) return false;
    }
    if (turn) this.turns++;
    this.chains.push({
      start: event.seq,
      end: event.seq,
      turn,
      frames: [],
      prefix: this.carry.prefix(),
    });
    if (this.window.kind !== "after") {
      while (this.turns > this.keepTurns) this.dropOldestTurn();
      while (this.chains.length > MAX_KEPT_CHAINS) this.popFront();
    }
    return true;
  }

  private popFront(): Chain | undefined {
    const chain = this.chains.shift();
    if (chain?.turn) this.turns--;
    return chain;
  }

  private dropOldestTurn() {
    this.popFront();
    while (this.chains[0] && !this.chains[0].turn) this.popFront();
  }

  private track(seq: number, frames: Frame[]) {
    for (const frame of frames) {
      const type = frame.event.type;
      if (type === "RUN_STARTED") this.run = String(frame.event.runId);
      else if (type === "RUN_FINISHED" || type === "RUN_ERROR") {
        const ended = this.run;
        this.run = null;
        if (ended !== null && this.window.kind === "after" && seq <= this.window.after) {
          this.anchor = { seq, runId: ended };
        }
      }
    }
  }

  finish(): Page {
    const head = this.head;
    const open = this.projector.runOpen && !this.capped;
    let anchor: Page["anchor"];
    let start: number;
    let end: number;
    const w = this.window;
    if (w.kind === "after") {
      anchor = this.anchor;
      if (open) this.chains.pop();
      this.fitNewest();
      const first = this.chains[0];
      const last = this.chains.at(-1);
      [start, end] = first && last ? [first.start, last.end] : [w.after + 1, w.after];
    } else if (w.kind === "turns") {
      end = this.closeBack(w.before !== undefined || open);
      while (this.turns > w.turns) this.dropOldestTurn();
      this.fitOldest();
      start = this.chains[0]?.start ?? end + 1;
    } else {
      end = this.closeBack(w.before !== undefined || open);
      while (this.chains[0] && this.chains[0].end < w.since) this.popFront();
      while (this.turns > Math.max(1, this.limits.maxTurns)) this.dropOldestTurn();
      this.fitOldest();
      start = this.chains[0]?.start ?? end + 1;
    }
    const earlier = start > 1;
    const carry =
      earlier && w.kind !== "after"
        ? this.carry.carry(this.chains[0]?.prefix ?? this.tail ?? this.carry.prefix())
        : undefined;
    return {
      start,
      end,
      head,
      earlier,
      frames: this.chains.flatMap((c) => c.frames),
      ...(anchor ? { anchor } : {}),
      ...(carry ? { carry } : {}),
    };
  }

  private closeBack(exclude: boolean): number {
    if (exclude) {
      const last = this.chains.pop();
      if (last) {
        if (last.turn) this.turns--;
        this.tail = last.prefix;
        return last.start - 1;
      }
    }
    return this.chains.at(-1)?.end ?? this.lastSeq;
  }

  private fitOldest() {
    let total = this.chains.reduce((n, c) => n + chainBytes(c), 0);
    while (this.chains.length > 1 && total > this.limits.maxPageBytes) {
      const oldest = this.popFront();
      if (oldest) total -= chainBytes(oldest);
    }
  }

  private fitNewest() {
    let total = this.chains.reduce((n, c) => n + chainBytes(c), 0);
    while (this.chains.length > 1 && total > this.limits.maxPageBytes) {
      const newest = this.chains.pop();
      if (newest) total -= chainBytes(newest);
    }
  }
}

const clamp = (n: number, low: number, high: number): number => Math.min(Math.max(n, low), high);

/** One read of `events`, in order, until the fold is done. */
export function readPage(
  info: ThreadInfo,
  events: readonly Event[],
  window: Window,
  limits: HistoryLimits,
  head: number,
): Page {
  const history = new History(info, window, limits, head);
  for (const event of events) if (!history.feed(event)) break;
  return history.finish();
}
