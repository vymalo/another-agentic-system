import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  CONTEXT_UNITS,
  CONTEXT_WORDS,
  EDGE_UNITS,
  MAX_KEY,
  MAX_MESSAGE_HITS,
  MAX_QUERY_LENGTH,
  MAX_TERMS,
  MAX_WORDS,
  MIN_TERM,
  parseQuery,
  type Searchable,
  searchableMessages,
  searchThreads,
  snippet,
  WINDOW_UNITS,
  wordsOf,
} from "./search";

/**
 * The conformance vectors of docs/api/thread-search-v1.md, in `docs/api/examples/thread-search/`. This file holds the mock's
 * implementation (`search.ts`, written from the page and importing nothing of `web/src`) to every one of them; the Rust core
 * (`orch_core::search`) reads the same files. A test also holds the page's own numbers to the code.
 */

const DIR = path.resolve(import.meta.dirname, "../../docs/api/examples/thread-search");
const PAGE = readFileSync(
  path.resolve(import.meta.dirname, "../../docs/api/thread-search-v1.md"),
  "utf8",
);
const read = <T>(file: string): T => JSON.parse(readFileSync(path.join(DIR, file), "utf8")) as T;

type Text = string | { repeat: string; times: number; suffix?: string };
const expand = (t: Text): string =>
  typeof t === "string" ? t : t.repeat.repeat(t.times) + (t.suffix ?? "");

describe("words and keys (fold.json)", () => {
  const file = read<{
    cases: {
      name: string;
      text: Text;
      words?: { start: number; end: number; key: Text | null }[];
      wordCount?: number;
      lastKey?: string;
    }[];
  }>("fold.json");

  it("has the cases the page names", () => {
    expect(file.cases.length).toBeGreaterThan(20);
  });
  for (const c of file.cases) {
    it(c.name, () => {
      const got = wordsOf(expand(c.text));
      if (c.wordCount !== undefined) {
        expect(got.words.length).toBe(c.wordCount);
        expect(got.more).toBe(true);
        expect(got.words.at(-1)?.key).toBe(c.lastKey);
        return;
      }
      expect(got.words).toEqual(
        (c.words ?? []).map((w) => ({
          start: w.start,
          end: w.end,
          key: w.key === null ? null : expand(w.key),
        })),
      );
    });
  }
});

describe("the query (query.json)", () => {
  const file = read<{
    cases: { name: string; q: Text; terms?: Text[]; error?: string }[];
  }>("query.json");
  for (const c of file.cases) {
    it(c.name, () => {
      const got = parseQuery(expand(c.q));
      if (c.error) expect(got).toEqual({ ok: false, code: c.error });
      else expect(got).toEqual({ ok: true, terms: (c.terms ?? []).map(expand) });
    });
  }
});

describe("the snippet (snippet.json)", () => {
  const file = read<{
    cases: { name: string; text: Text; terms: string[]; expect: unknown }[];
  }>("snippet.json");
  for (const c of file.cases) {
    it(c.name, () => {
      expect(
        snippet(
          expand(c.text),
          c.terms.map((t) => t),
        ),
      ).toEqual(c.expect);
    });
  }
  it("never splits a surrogate pair and marks whole words", () => {
    for (const c of file.cases) {
      const got = snippet(expand(c.text), c.terms);
      if (!got) continue;
      for (const m of got.matches) {
        const word = got.text.slice(m.start, m.end);
        expect(word.length).toBe(m.end - m.start);
        // a well-formed string: no lone surrogate at either end
        expect(word).toBe(word.toWellFormed());
      }
      expect(got.text).toBe(got.text.toWellFormed());
    }
  });
});

describe("a person's threads (corpus.json and queries/)", () => {
  type CorpusEvent = {
    seq: number;
    at: string;
    kind: string;
    text?: string;
    messageId?: string;
    final?: boolean;
    purpose?: string;
  };
  const corpus = read<{
    threads: {
      id: string;
      title: string;
      description?: string | null;
      archived: boolean;
      edit: boolean;
      copiedThrough?: number;
      events: CorpusEvent[];
    }[];
  }>("corpus.json");
  const threads: Searchable[] = corpus.threads.map((t) => ({
    id: t.id,
    title: t.title,
    description: t.description ?? null,
    archived: t.archived,
    edit: t.edit,
    ...(t.copiedThrough !== undefined ? { copiedThrough: t.copiedThrough } : {}),
    messages: searchableMessages(t.events),
  }));

  type Step = {
    request: Record<string, unknown>;
    expect: { terms?: string[]; results?: unknown[]; hasNext?: boolean; error?: string };
  };
  const files = readdirSync(path.join(DIR, "queries")).filter((f) => f.endsWith(".json"));
  it("has the queries the page names", () => {
    expect(files.length).toBeGreaterThan(15);
  });
  for (const file of files) {
    const query = read<{ name: string; steps: Step[] }>(`queries/${file}`);
    it(`${file}: ${query.name}`, () => {
      let cursor: string | undefined;
      for (const step of query.steps) {
        const request = { ...step.request } as Record<string, unknown> & { q: Text };
        if (request.cursor === "$previous") request.cursor = cursor;
        const got = searchThreads(threads, { ...request, q: expand(request.q) } as never);
        if (step.expect.error) {
          expect(got).toEqual({ ok: false, code: step.expect.error });
          cursor = undefined;
          continue;
        }
        expect(got.ok).toBe(true);
        if (!got.ok) return;
        expect(got.page.terms).toEqual(step.expect.terms);
        expect(got.page.results).toEqual(step.expect.results);
        expect(got.page.next !== undefined).toBe(step.expect.hasNext);
        cursor = got.page.next;
      }
    });
  }
});

describe("the page and the code agree", () => {
  const constants: Record<string, number> = {
    MAX_QUERY_LENGTH,
    MAX_TERMS,
    MIN_TERM,
    MAX_KEY,
    MAX_WORDS,
    MAX_MESSAGE_HITS,
    CONTEXT_WORDS,
    CONTEXT_UNITS,
    WINDOW_UNITS,
    EDGE_UNITS,
  };
  for (const [name, value] of Object.entries(constants)) {
    it(`${name} is ${value} on the page`, () => {
      const row = PAGE.split("\n").find((l) => l.startsWith(`| \`${name}\` |`));
      expect(row, `a row for ${name}`).toBeDefined();
      expect(Number((row ?? "").split("|")[2]?.trim())).toBe(value);
    });
  }
  it("names the codes it answers", () => {
    for (const code of [
      "query_empty",
      "query_too_long",
      "bad_cursor",
      "bad_request",
      "rate_limited",
      "search_unavailable",
    ]) {
      expect(PAGE, code).toContain(code);
    }
  });
});

describe("the search, on the rules of the page", () => {
  const thread = (over: Partial<Searchable> & { id: string }): Searchable => ({
    title: "t",
    archived: false,
    edit: false,
    messages: [],
    ...over,
  });
  it("a search with no term finds nothing, whatever the threads", () => {
    const out = searchThreads([thread({ id: "a", title: "a a a" })], { q: "a" });
    expect(out).toEqual({ ok: true, page: { terms: [], results: [] } });
  });
  it("the title beats the description beats a message, then the newer thread first", () => {
    const out = searchThreads(
      [
        thread({ id: "1", messages: [{ seq: 1, role: "person", at: "x", text: "zebra" }] }),
        thread({ id: "2", description: "a zebra" }),
        thread({ id: "3", title: "zebra" }),
        thread({ id: "4", messages: [{ seq: 1, role: "person", at: "x", text: "zebra" }] }),
      ],
      { q: "zebra" },
    );
    expect(out.ok && out.page.results.map((r) => [r.id, r.rank])).toEqual([
      ["3", 2],
      ["2", 1],
      ["4", 0],
      ["1", 0],
    ]);
  });
});
