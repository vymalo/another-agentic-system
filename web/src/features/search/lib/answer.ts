import type { ApiSearchHit, ApiThreadHit } from "@/lib/api/types";

/** What the page reads of an answer: the terms, the threads (each once) and the cursor. */
export type Answer = { terms: string[]; results: ApiThreadHit[]; next?: string };

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

const isInt = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v);

function isSnippet(v: unknown): boolean {
  if (!isRecord(v)) return false;
  if (typeof v.text !== "string") return false;
  if (typeof v.cutStart !== "boolean" || typeof v.cutEnd !== "boolean") return false;
  return (
    Array.isArray(v.matches) &&
    v.matches.every((m) => isRecord(m) && isInt(m.start) && isInt(m.end))
  );
}

function isHit(v: unknown): v is ApiSearchHit {
  if (!isRecord(v)) return false;
  if (v.field !== "title" && v.field !== "description" && v.field !== "message") return false;
  if (v.seq !== undefined && !isInt(v.seq)) return false;
  return isSnippet(v.snippet);
}

function isResult(v: unknown): v is ApiThreadHit {
  if (!isRecord(v)) return false;
  const thread = v.thread;
  if (!isRecord(thread) || typeof thread.id !== "string" || typeof thread.title !== "string") {
    return false;
  }
  if (!isInt(v.messageHits) || !Array.isArray(v.hits)) return false;
  return v.hits.every(isHit);
}

/**
 * The answer as the page needs it, or `null` when it is not the shape the contract promises (a proxy's page, a newer server): the
 * page then says the search failed instead of throwing while it draws. Written by hand, no dependency; it checks what is drawn.
 */
export function readAnswer(data: unknown): Answer | null {
  if (!isRecord(data) || !isRecord(data.query)) return null;
  const { terms } = data.query;
  if (!Array.isArray(terms) || !terms.every((t) => typeof t === "string")) return null;
  if (!Array.isArray(data.results) || !data.results.every(isResult)) return null;
  if (data.next !== undefined && typeof data.next !== "string") return null;
  return {
    terms: terms as string[],
    results: dedupe(data.results as ApiThreadHit[]),
    ...(typeof data.next === "string" ? { next: data.next } : {}),
  };
}

/** Each thread once, the first kept: a thread renamed between two pages can come twice, and a React key must not. */
export function dedupe(results: ApiThreadHit[]): ApiThreadHit[] {
  const seen = new Set<string>();
  return results.filter((r) => {
    if (seen.has(r.thread.id)) return false;
    seen.add(r.thread.id);
    return true;
  });
}
