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
  readRequest,
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
    text?: Text;
    messageId?: string;
    final?: boolean;
    purpose?: string;
  };
  const corpus = read<{
    threads: {
      id: string;
      title: Text;
      description?: Text | null;
      archived: boolean;
      edit: boolean;
      copiedThrough?: number;
      events: CorpusEvent[];
    }[];
  }>("corpus.json");
  const threads: Searchable[] = corpus.threads.map((t) => ({
    id: t.id,
    title: expand(t.title),
    description:
      t.description === undefined || t.description === null ? null : expand(t.description),
    archived: t.archived,
    edit: t.edit,
    ...(t.copiedThrough !== undefined ? { copiedThrough: t.copiedThrough } : {}),
    messages: searchableMessages(
      t.events.map(({ text, ...rest }) => ({
        ...rest,
        ...(text !== undefined ? { text: expand(text) } : {}),
      })),
    ),
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

describe("a request (requests.json)", () => {
  const file = read<{
    cases: {
      name: string;
      headers?: Record<string, string>;
      body: string;
      expect: { status: number; code?: string; terms?: string[] };
    }[];
  }>("requests.json");
  const threads: Searchable[] = [];
  it("has the cases the page names", () => {
    expect(file.cases.length).toBeGreaterThanOrEqual(60);
  });
  for (const c of file.cases) {
    it(c.name, () => {
      const headers = c.headers ?? { "content-type": "application/json" };
      const read = readRequest(headers["content-type"], c.body);
      let got: { status: number; code?: string; terms?: string[] };
      if (!read.ok) got = { status: read.status, ...(read.code ? { code: read.code } : {}) };
      else {
        const out = searchThreads(threads, read.request);
        got = out.ok ? { status: 200, terms: out.page.terms } : { status: 400, code: out.code };
      }
      expect(got.status).toBe(c.expect.status);
      expect(got.code).toBe(c.expect.code);
      if (c.expect.terms) expect(got.terms).toEqual(c.expect.terms);
    });
  }
});

describe("a cursor is a position that carries no authority", () => {
  const id = (n: number) => `0190aaaa-0000-7000-8000-00000000000${n}`;
  const threads: Searchable[] = [1, 2, 3].map((n) => ({
    id: id(n),
    title: "Zebra",
    archived: false,
    edit: false,
    messages: [],
  }));
  const encode = (v: unknown) => Buffer.from(JSON.stringify(v)).toString("base64url");
  const first = searchThreads(threads, { q: "zebra", limit: 2 });
  const next = first.ok ? (first.page.next as string) : "";
  const [rank, at, print] = JSON.parse(Buffer.from(next, "base64url").toString("utf8")) as [
    number,
    string,
    string,
  ];
  it("is accepted for the same terms and filters, however the query is spelled", () => {
    const out = searchThreads(threads, { q: "  ZEBRA!! ", limit: 2, cursor: next });
    expect(out.ok && out.page.results.length).toBe(1);
  });
  it("is bad_cursor when the position is not valid", () => {
    for (const bad of [
      [7, at, print],
      [3, at, print],
      [-1, at, print],
      [1.5, at, print],
      ["0", at, print],
      [rank, at.toUpperCase(), print],
      [rank, "not-a-uuid", print],
      [rank, "", print],
      [rank, at],
      [rank, at, print, 1],
    ]) {
      expect(searchThreads(threads, { q: "zebra", limit: 2, cursor: encode(bad) })).toEqual({
        ok: false,
        code: "bad_cursor",
      });
    }
    expect(searchThreads(threads, { q: "zebra", cursor: "x".repeat(600) })).toEqual({
      ok: false,
      code: "bad_cursor",
    });
  });
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
