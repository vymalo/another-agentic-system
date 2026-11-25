/**
 * A person's memory as the mock reads it (docs/api/memory-v1.md, ADR 0091), written from the
 * contract and not from the orchestrator's `orch-core::memory`, so the two are two readings of one
 * document, held together by the vectors in docs/api/examples/memory/ and not by shared code.
 */

export const MAX_NOTE_BYTES = 1024;
export const MAX_NOTES = 200;
export const MAX_READ_NOTES = 50;
export const MAX_QUERY_BYTES = 128;

export type MemoryTool = "memory_read" | "memory_save" | "memory_forget";
export type Grant = { read: boolean; write: boolean };
export type ThreadSwitch = "on" | "off";
export type MemoryErrorCode =
  | "memory_off"
  | "no_grant"
  | "not_main_caller"
  | "too_long"
  | "too_many"
  | "invalid";
/** A refusal: `unknown tool` for a tool that is not listed, else the contract's code. */
export type Refusal = "unknown tool" | MemoryErrorCode;

export type Note = { id: string; text: string; savedAt: string; writtenBy: string };

export class MemoryRefused extends Error {
  constructor(readonly code: MemoryErrorCode) {
    super(code); // a code, never a note's text
  }
}

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const byteLength = (s: string): number => new TextEncoder().encode(s).length;
const allows = (g: Grant, tool: MemoryTool): boolean => (tool === "memory_read" ? g.read : g.write);

/** Whether `tools/list` shows the tool: a main caller, the switch on, the grant held. */
export function listed(
  tool: MemoryTool,
  sw: ThreadSwitch,
  main: boolean,
  grant: Grant | undefined,
): boolean {
  return main && sw === "on" && grant !== undefined && allows(grant, tool);
}

/** `null` when the call may run; else why not, in the contract's order. */
export function checkCall(
  tool: MemoryTool,
  sw: ThreadSwitch,
  main: boolean,
  grant: Grant | undefined,
): Refusal | null {
  if (grant === undefined) return "unknown tool";
  if (sw === "off") return "memory_off";
  if (!allows(grant, tool)) return "no_grant";
  if (!main) return "not_main_caller";
  return null;
}

export type ReadResult = { notes: Note[]; total: number; truncated: boolean };

/** One person's notes, in the order saved. */
export class Memory {
  private notes: Note[] = [];

  get length(): number {
    return this.notes.length;
  }

  save(input: unknown, writtenBy: string, id: string, savedAt: string): Note {
    const raw = isRecord(input) ? input.text : undefined;
    if (typeof raw !== "string") throw new MemoryRefused("invalid");
    const text = raw.trim();
    if (text === "") throw new MemoryRefused("invalid");
    if (byteLength(text) > MAX_NOTE_BYTES) throw new MemoryRefused("too_long");
    if (this.notes.length >= MAX_NOTES) throw new MemoryRefused("too_many");
    const note = { id, text, savedAt, writtenBy };
    this.notes.push(note);
    return note;
  }

  read(input: unknown): ReadResult {
    if (!isRecord(input)) throw new MemoryRefused("invalid");
    const q = input.query;
    if (q !== undefined && q !== null && (typeof q !== "string" || byteLength(q) > MAX_QUERY_BYTES))
      throw new MemoryRefused("invalid");
    let limit = MAX_READ_NOTES;
    if (input.limit !== undefined && input.limit !== null) {
      if (typeof input.limit !== "number" || !Number.isInteger(input.limit))
        throw new MemoryRefused("invalid");
      limit = Math.min(Math.max(input.limit, 1), MAX_READ_NOTES);
    }
    const needle = typeof q === "string" ? q.toLowerCase() : null;
    const matched = [...this.notes]
      .reverse()
      .filter((n) => needle === null || n.text.toLowerCase().includes(needle));
    const notes = matched.slice(0, limit);
    return { notes, total: matched.length, truncated: matched.length > notes.length };
  }

  /** `false` for an id that is not there; never says whose it is. */
  forget(input: unknown): boolean {
    const id = isRecord(input) ? input.id : undefined;
    if (typeof id !== "string" || id === "") throw new MemoryRefused("invalid");
    const before = this.notes.length;
    this.notes = this.notes.filter((n) => n.id !== id);
    return this.notes.length !== before;
  }
}

/** A person's grants: both `false` by default, one entry per agent they touched. */
export class MemorySettings {
  readonly default: Grant = { read: false, write: false };
  readonly agents: Record<string, Grant> = {};

  grant(agent: string): Grant | undefined {
    return this.agents[agent];
  }

  /** Applies a `PUT` body; throws `MemoryRefused("invalid")` and changes nothing when it is not one. */
  apply(body: unknown): string {
    if (!isRecord(body)) throw new MemoryRefused("invalid");
    const { agent, read, write } = body;
    if (typeof agent !== "string" || agent === "") throw new MemoryRefused("invalid");
    if (typeof read !== "boolean" || typeof write !== "boolean") throw new MemoryRefused("invalid");
    this.agents[agent] = { read, write };
    return agent;
  }

  toJSON() {
    return { default: this.default, agents: this.agents };
  }
}
