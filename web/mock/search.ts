/*
 * Search of a person's own threads, written from docs/api/thread-search-v1.md and from nothing else: this file imports nothing of
 * `web/src`, because it is the SECOND implementation of the contract (the first is `orch_core::search`, in Rust). A third client
 * would write a third. `search.test.ts` holds both to the same vectors in docs/api/examples/thread-search/.
 *
 * Everything here is exact and deterministic: the words, their keys, the terms of a query, which documents match, the snippets, the
 * offsets (UTF-16 code units, as JavaScript counts them) and the order. Nothing depends on a ranking function.
 */

/** The longest query, in Unicode scalar values, after it is trimmed. */
export const MAX_QUERY_LENGTH = 200;
/** The most terms of a query that are used. */
export const MAX_TERMS = 8;
/** The most words of one document that are searched. */
export const MAX_WORDS = 3000;
/** The longest key, in code points: a longer word is not indexed, and a longer term is cut. */
export const MAX_KEY = 64;
/** A term shorter than this (in code points) is dropped. */
export const MIN_TERM = 2;
/** The words before the first match that a snippet starts with, at most. */
export const CONTEXT_WORDS = 6;
/** A snippet does not reach further back than this before the first match (UTF-16 units). */
export const CONTEXT_UNITS = 100;
/** A snippet is about this long (UTF-16 units). */
export const WINDOW_UNITS = 200;
/** Text that is left over at the very start or end of the document is kept up to this much (UTF-16 units). */
export const EDGE_UNITS = 20;
/** The most message hits a thread shows. */
export const MAX_MESSAGE_HITS = 3;

export interface Word {
  /** UTF-16 offset of the first unit of the word. */
  start: number;
  /** UTF-16 offset after the last unit. */
  end: number;
  /** The folded word, or `null` when it has none (it folds to nothing or is longer than 64 code points). */
  key: string | null;
}

const WORD = /[\p{L}\p{M}\p{N}]+/gu;

/** The folded form of one word, with no length limit; `""` when nothing is left. */
export function fold(word: string): string {
  return word
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/œ/g, "oe")
    .replace(/æ/g, "ae")
    .replace(/ß/g, "ss")
    .replace(/ς/g, "σ")
    .replace(/[^\p{L}\p{N}]/gu, "");
}

const codePoints = (s: string): number => Array.from(s).length;

/** The words of a text (the first {@link MAX_WORDS}), and whether there are more. */
export function wordsOf(text: string): { words: Word[]; more: boolean } {
  const words: Word[] = [];
  let more = false;
  for (const m of text.matchAll(WORD)) {
    if (words.length === MAX_WORDS) {
      more = true;
      break;
    }
    const key = fold(m[0]);
    words.push({
      start: m.index,
      end: m.index + m[0].length,
      key: key === "" || codePoints(key) > MAX_KEY ? null : key,
    });
  }
  return { words, more };
}

const isBlankChar = (c: string) => c === " " || c === "\t" || c === "\n" || c === "\r";

/** Trims U+0020, U+0009, U+000A and U+000D, and only those. */
export function trimWhitespace(s: string): string {
  let a = 0;
  let b = s.length;
  while (a < b && isBlankChar(s.charAt(a))) a++;
  while (b > a && isBlankChar(s.charAt(b - 1))) b--;
  return s.slice(a, b);
}

export type QueryError = "query_empty" | "query_too_long";

export type ParsedQuery = { ok: true; terms: string[] } | { ok: false; code: QueryError };

export function parseQuery(q: string): ParsedQuery {
  const trimmed = trimWhitespace(q);
  if (trimmed === "") return { ok: false, code: "query_empty" };
  if (codePoints(trimmed) > MAX_QUERY_LENGTH) return { ok: false, code: "query_too_long" };
  const terms: string[] = [];
  for (const m of trimmed.matchAll(WORD)) {
    let key = fold(m[0]);
    if (codePoints(key) > MAX_KEY) key = Array.from(key).slice(0, MAX_KEY).join("");
    if (codePoints(key) < MIN_TERM || terms.includes(key)) continue;
    terms.push(key);
    if (terms.length === MAX_TERMS) break;
  }
  return { ok: true, terms };
}

/** Whether a text holds every term as the start of one of its words. */
export function matches(text: string, terms: string[]): boolean {
  if (terms.length === 0) return false;
  const keys = wordsOf(text)
    .words.map((w) => w.key)
    .filter((k): k is string => k !== null);
  return terms.every((t) => keys.some((k) => k.startsWith(t)));
}

export interface Snippet {
  text: string;
  cutStart: boolean;
  cutEnd: boolean;
  matches: { start: number; end: number }[];
}

/** U+0020, U+0009, U+000A and U+000D runs become one space, and the ends are trimmed. */
export function collapse(text: string): string {
  return trimWhitespace(text.replace(/[ \t\n\r]+/g, " "));
}

/** The snippet of a text for these terms, or `null` when no word of it starts with a term. */
export function snippet(text: string, terms: string[]): Snippet | null {
  const flat = collapse(text);
  const { words, more } = wordsOf(flat);
  const hit = (w: Word) => w.key !== null && terms.some((t) => (w.key as string).startsWith(t));
  const at = words.findIndex(hit);
  if (at < 0) return null;
  const w = (i: number): Word => words[i] as Word;
  let first = at;
  for (let j = Math.max(0, at - CONTEXT_WORDS); j < at; j++) {
    if (w(j).start >= w(at).start - CONTEXT_UNITS) {
      first = j;
      break;
    }
  }
  let start = w(first).start;
  let last = at;
  for (let j = at + 1; j < words.length; j++) {
    if (w(j).end <= start + WINDOW_UNITS) last = j;
    else break;
  }
  let end = w(last).end;
  const shown = words.slice(first, last + 1).filter(hit);
  if (first === 0 && start <= EDGE_UNITS) start = 0;
  if (last === words.length - 1 && !more && flat.length - end <= EDGE_UNITS) end = flat.length;
  return {
    text: flat.slice(start, end),
    cutStart: start > 0,
    cutEnd: end < flat.length,
    matches: shown.map((w) => ({ start: w.start - start, end: w.end - start })),
  };
}

export interface LogEvent {
  seq: number;
  at: string;
  kind: string;
  text?: string;
  messageId?: string;
  final?: boolean;
  purpose?: string;
}

export interface Message {
  seq: number;
  role: "person" | "agent";
  at: string;
  text: string;
}

/**
 * The messages of a log that are searched: a person's `user_message`, and an agent's `agent_message` that is final and is not
 * working text. A later final message with the same `messageId` replaces the earlier one (it is searched, or not, as itself).
 */
export function searchableMessages(events: LogEvent[]): Message[] {
  const out: Message[] = [];
  for (const e of [...events].sort((a, b) => a.seq - b.seq)) {
    if (e.kind === "user_message") {
      out.push({ seq: e.seq, role: "person", at: e.at, text: e.text ?? "" });
    } else if (e.kind === "agent_message" && e.final === true) {
      if (e.messageId !== undefined) {
        for (let i = out.length - 1; i >= 0; i--) {
          const prior = out[i] as Message & { id?: string };
          if (prior.role === "agent" && prior.id === e.messageId) out.splice(i, 1);
        }
      }
      if (e.purpose !== "working") {
        const m: Message & { id?: string } = {
          seq: e.seq,
          role: "agent",
          at: e.at,
          text: e.text ?? "",
        };
        if (e.messageId !== undefined) m.id = e.messageId;
        out.push(m);
      }
    }
  }
  return out.map(({ seq, role, at, text }) => ({ seq, role, at, text }));
}

export interface Searchable {
  id: string;
  title: string;
  description?: string | null;
  archived: boolean;
  /** Made by an edit of a message: a branch, listed only when branches are asked for. */
  edit: boolean;
  /** The events up to this one were copied from a parent that still exists: they give no message hit. */
  copiedThrough?: number;
  messages: Message[];
}

export type ArchivedFilter = "exclude" | "only" | "include";

export interface SearchRequest {
  q: string;
  limit?: number;
  cursor?: string;
  archived?: ArchivedFilter;
  branches?: "include";
}

export interface Hit {
  field: "title" | "description" | "message";
  seq?: number;
  role?: "person" | "agent";
  at?: string;
  snippet: Snippet;
}

export interface ThreadHit {
  id: string;
  rank: 0 | 1 | 2;
  hits: Hit[];
  messageHits: number;
}

export interface SearchPage {
  terms: string[];
  results: ThreadHit[];
  next?: string;
}

export type SearchOutcome =
  | { ok: true; page: SearchPage }
  | { ok: false; code: QueryError | "bad_cursor" | "bad_request" };

const DEFAULT_LIMIT = 20;
const MAX_LIMIT = 50;
/** The longest cursor accepted: a longer one cannot be one a server made. */
const MAX_CURSOR_LENGTH = 512;
/** A thread id: a lower-case hyphenated UUID, the form the ids are ordered in. */
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/**
 * The cursor is this server's own business; this one is a base64url JSON of where the page ended and what it was asked. It is a
 * position (rank, thread id) checked against the terms and the filters of the request, and carries no authority: a position that is
 * not valid (a rank that is not 0, 1 or 2, an id that is not a lower-case hyphenated UUID) is `bad_cursor`.
 */
function fingerprint(terms: string[], archived: string, branches: boolean): string {
  return JSON.stringify([terms, archived, branches]);
}

function encodeCursor(rank: number, id: string, fp: string): string {
  return Buffer.from(JSON.stringify([rank, id, fp])).toString("base64url");
}

function decodeCursor(cursor: string, fp: string): { rank: number; id: string } | null {
  try {
    const v = JSON.parse(Buffer.from(cursor, "base64url").toString("utf8"));
    if (
      Array.isArray(v) &&
      v.length === 3 &&
      (v[0] === 0 || v[0] === 1 || v[0] === 2) &&
      typeof v[1] === "string" &&
      UUID.test(v[1]) &&
      v[2] === fp
    ) {
      return { rank: v[0], id: v[1] };
    }
  } catch {
    // falls through
  }
  return null;
}

export function searchThreads(threads: Searchable[], req: SearchRequest): SearchOutcome {
  const parsed = parseQuery(req.q);
  if (!parsed.ok) return parsed;
  const limit = req.limit ?? DEFAULT_LIMIT;
  if (!Number.isInteger(limit) || limit < 1 || limit > MAX_LIMIT)
    return { ok: false, code: "bad_request" };
  const archived = req.archived ?? "include";
  if (archived !== "exclude" && archived !== "only" && archived !== "include")
    return { ok: false, code: "bad_request" };
  if (req.branches !== undefined && req.branches !== "include")
    return { ok: false, code: "bad_request" };
  const branches = req.branches === "include";
  const { terms } = parsed;
  const fp = fingerprint(terms, archived, branches);
  let after: { rank: number; id: string } | null = null;
  if (req.cursor !== undefined) {
    after = req.cursor.length > MAX_CURSOR_LENGTH ? null : decodeCursor(req.cursor, fp);
    if (after === null) return { ok: false, code: "bad_cursor" };
  }
  const found: ThreadHit[] = [];
  if (terms.length > 0) {
    for (const t of threads) {
      if (!branches && t.edit) continue;
      if (archived === "exclude" && t.archived) continue;
      if (archived === "only" && !t.archived) continue;
      const hits: Hit[] = [];
      let rank: 0 | 1 | 2 = 0;
      const title = matches(t.title, terms) ? snippet(t.title, terms) : null;
      if (title) {
        hits.push({ field: "title", snippet: title });
        rank = 2;
      }
      const description =
        t.description && matches(t.description, terms) ? snippet(t.description, terms) : null;
      if (description) {
        hits.push({ field: "description", snippet: description });
        if (rank === 0) rank = 1;
      }
      const messages = t.messages
        .filter((m) => t.copiedThrough === undefined || m.seq > t.copiedThrough)
        .filter((m) => matches(m.text, terms))
        .sort((a, b) => b.seq - a.seq);
      for (const m of messages.slice(0, MAX_MESSAGE_HITS)) {
        const s = snippet(m.text, terms);
        if (s) hits.push({ field: "message", seq: m.seq, role: m.role, at: m.at, snippet: s });
      }
      if (hits.length > 0) found.push({ id: t.id, rank, hits, messageHits: messages.length });
    }
  }
  found.sort((a, b) => b.rank - a.rank || (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));
  const rest = after
    ? found.filter(
        (h) =>
          h.rank < (after as { rank: number }).rank ||
          (h.rank === (after as { rank: number }).rank && h.id < (after as { id: string }).id),
      )
    : found;
  const results = rest.slice(0, limit);
  const page: SearchPage = { terms, results };
  const lastOfPage = results[results.length - 1];
  if (rest.length > limit && lastOfPage)
    page.next = encodeCursor(lastOfPage.rank, lastOfPage.id, fp);
  return { ok: true, page };
}

/** What reading a request can answer before any search: 415 for the media type, 400 `bad_request` for the shape. */
export type RequestOutcome =
  | { ok: true; request: SearchRequest }
  | { ok: false; status: 400 | 415; code?: "bad_request" };

const MEMBERS = ["q", "limit", "cursor", "archived", "branches"];
/** A JSON integer literal: no fraction, no exponent. */
const INTEGER_LITERAL = /^-?(0|[1-9][0-9]*)$/;

/**
 * The members of the top-level object of a JSON text that has already parsed: each key in the order written (so a key twice shows),
 * and the source of each member whose value is a number or another bare literal (so `20.0` can be told from `20`). A tokenizer,
 * not a parser: it relies on the text being valid.
 */
function scanMembers(text: string): { keys: string[]; literals: Map<string, string> } {
  const keys: string[] = [];
  const literals = new Map<string, string>();
  let depth = 0;
  let expectKey = false;
  let key: string | undefined;
  let afterColon = false;
  for (const m of text.matchAll(/"(?:[^"\\]|\\.)*"|[{}[\],:]|[^\s{}[\],:"]+/g)) {
    const token = m[0];
    if (token === "{" || token === "[") {
      depth++;
      expectKey = depth === 1 && token === "{";
      afterColon = false;
    } else if (token === "}" || token === "]") {
      depth--;
      afterColon = false;
    } else if (token === ",") {
      expectKey = depth === 1;
      afterColon = false;
    } else if (token === ":") {
      afterColon = depth === 1;
    } else if (depth === 1 && expectKey && token.startsWith('"')) {
      key = JSON.parse(token) as string;
      keys.push(key);
      expectKey = false;
    } else if (depth === 1 && afterColon && key !== undefined) {
      if (!token.startsWith('"')) literals.set(key, token);
      afterColon = false;
    }
  }
  return { keys, literals };
}

/**
 * Reads a request as docs/api/thread-search-v1.md says: the media type is exactly `application/json` (parameters such as `charset`
 * allowed, case ignored), the body is one JSON object with only `q` (a string, required), `limit` (a JSON integer literal),
 * `cursor`, `archived` and `branches` (strings), no member twice, no `null`, no lone surrogate in any string. What it refuses is
 * 415 or 400 `bad_request`; the query, the range of `limit`, the filters and the cursor are judged by {@link searchThreads}.
 */
export function readRequest(contentType: string | undefined, body: string): RequestOutcome {
  const bad: RequestOutcome = { ok: false, status: 400, code: "bad_request" };
  const media = (contentType ?? "").split(";")[0]?.trim().toLowerCase();
  if (media !== "application/json") return { ok: false, status: 415 };
  let value: unknown;
  try {
    value = JSON.parse(body);
  } catch {
    return bad;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) return bad;
  const { keys, literals } = scanMembers(body);
  if (new Set(keys).size !== keys.length) return bad;
  if (keys.some((k) => !MEMBERS.includes(k) || !k.isWellFormed())) return bad;
  const r = value as Record<string, unknown>;
  if (typeof r.q !== "string" || !r.q.isWellFormed()) return bad;
  for (const name of ["cursor", "archived", "branches"] as const) {
    const v = r[name];
    if (v !== undefined && (typeof v !== "string" || !v.isWellFormed())) return bad;
  }
  if (r.limit !== undefined) {
    if (typeof r.limit !== "number") return bad;
    if (!INTEGER_LITERAL.test(literals.get("limit") ?? "")) return bad;
  }
  return { ok: true, request: r as unknown as SearchRequest };
}
