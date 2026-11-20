import { readFileSync } from "node:fs";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  addShared,
  audienceSet,
  handlePrompts,
  MAX_AUDIENCE,
  MAX_AUDIENCE_VALUE,
  MAX_BODY_BYTES,
  MAX_DESCRIPTION,
  MAX_NAME,
  MAX_PROMPTS,
  MAX_RENDERED_BYTES,
  MAX_REQUEST_BYTES,
  MAX_TITLE,
  MAX_VARIABLES,
  newShelf,
  PAGE,
  parseBody,
  type Save,
  type Shelf,
  validateAnswers,
  validateSave,
} from "./prompts";
import { createMockServer } from "./server";

/**
 * The conformance vectors of docs/api/prompts-v1.md, in `docs/api/examples/prompts/`, and the answer blocks of the UI catalog page.
 * This file holds the mock's implementation (`prompts.ts`, written from the page and importing nothing of `web/src`) to every one
 * of them, twice: through the route function the mock's server calls, and through HTTP against the mock server. The Rust core
 * (`orch_core::prompt`) and the Rust API read the same files.
 */

const ROOT = path.resolve(import.meta.dirname, "../..");
const DIR = path.join(ROOT, "docs/api/examples/prompts");
const read = <T>(file: string): T => JSON.parse(readFileSync(path.join(DIR, file), "utf8")) as T;
const PAGE_TEXT = readFileSync(path.join(ROOT, "docs/api/prompts-v1.md"), "utf8");
const CATALOG_PAGE = readFileSync(path.join(ROOT, "docs/api/ui-catalog-v1.md"), "utf8");

type Json = unknown;
type Rec = Record<string, Json>;

/** `{"$concat": [piece, ...]}` is the pieces joined (a piece: a string or `{"$repeat": text, "times": n}`), anywhere in a vector. */
function expand(v: Json): Json {
  if (Array.isArray(v)) return v.map(expand);
  if (v !== null && typeof v === "object") {
    const o = v as Rec;
    if ("$concat" in o) {
      return (o.$concat as Json[])
        .map((p) =>
          typeof p === "string"
            ? p
            : String((p as Rec).$repeat).repeat((p as { times: number }).times),
        )
        .join("");
    }
    if ("$repeat" in o) return String(o.$repeat).repeat((o as { times: number }).times);
    return Object.fromEntries(Object.entries(o).map(([k, x]) => [k, expand(x)]));
  }
  return v;
}

// ---- the page and the code agree ----------------------------------------------------------------

describe("the page and the code agree", () => {
  const rows = new Map<string, number>();
  for (const m of PAGE_TEXT.matchAll(/^\| `([A-Z_]+)` \| (\d+) \|/gm)) {
    rows.set(m[1] as string, Number(m[2]));
  }
  it("holds every constant of the page to the code", () => {
    expect(Object.fromEntries(rows)).toEqual({
      MAX_PROMPTS,
      MAX_NAME,
      MAX_TITLE,
      MAX_DESCRIPTION,
      MAX_BODY_BYTES,
      MAX_VARIABLES,
      MAX_AUDIENCE,
      MAX_AUDIENCE_VALUE,
      MAX_RENDERED_BYTES,
      MAX_REQUEST_BYTES,
      PAGE,
    });
  });
});

// ---- the references and the pure rules ------------------------------------------------------------

describe("references (references.json)", () => {
  const file = read<{
    cases: {
      name: string;
      body: string;
      expect: { segments: (string | { ref: string })[] } | { code: string; offset: number };
    }[];
  }>("references.json");
  it("has the cases the page names", () => {
    expect(file.cases.length).toBeGreaterThan(30);
  });
  for (const c of file.cases) {
    it(c.name, () => {
      const got = parseBody(c.body);
      if ("segments" in c.expect) expect(got).toEqual({ segments: c.expect.segments });
      else expect(got).toEqual({ offset: c.expect.offset });
    });
  }
});

describe("the UI catalog's own blocks (ui-catalog-v1.md, Receiving an answer)", () => {
  const block = <T>(name: string): T => {
    const m = new RegExp(`\`\`\`json ${name}\\n([\\s\\S]*?)\`\`\``).exec(CATALOG_PAGE);
    if (!m) throw new Error(`the catalog page has no ${name} block`);
    return JSON.parse(m[1] as string) as T;
  };
  const prompt = (questions: Rec[]): Save => ({
    name: "p",
    title: "T",
    description: "",
    body: questions.map((q) => `{{${String(q.id)}}}`).join(" "),
    variables: questions,
    audience: [],
  });

  const vectors = block<{ question: Rec; cases: [Json[], string | null][] }[]>("vectors");
  it("passes every case of `vectors` as a one-variable prompt", () => {
    expect(vectors.reduce((n, v) => n + v.cases.length, 0)).toBe(133);
    for (const v of vectors) {
      expect(validateSave(prompt([v.question])), JSON.stringify(v.question)).toBeUndefined();
      for (const [values, rule] of v.cases) {
        const got = validateAnswers([v.question], [{ id: v.question.id, values }], false);
        expect(got?.rule ?? null, `${JSON.stringify(v.question)} ${JSON.stringify(values)}`).toBe(
          rule,
        );
      }
    }
  });

  it("passes every case of `whole-answers` that a prompt can have", () => {
    const cases =
      block<
        {
          freeInput: boolean;
          questions: Rec[];
          context: Rec;
          refusal: [string, string] | null;
        }[]
      >("whole-answers");
    let skipped = 0;
    for (const c of cases) {
      const members = Object.keys(c.context).filter((k) => k !== "answers" && k !== "note");
      // a prompt has no free box: a form that has one has no prompt, and an extra member is a 400 here
      if (c.freeInput || members.length > 0) {
        skipped += 1;
        continue;
      }
      const got = validateAnswers(c.questions, c.context.answers, "note" in c.context);
      expect(got ? [got.question, got.rule] : null, JSON.stringify(c)).toEqual(c.refusal);
    }
    expect(skipped).toBe(2);
  });

  it("refuses every question of `invalid-questions` as a bad variable", () => {
    const cases = block<{ because: string; question: Rec }[]>("invalid-questions");
    expect(cases.length).toBeGreaterThan(30);
    for (const c of cases) {
      expect(validateSave(prompt([c.question]))?.code, c.because).toBe("bad_variable");
    }
  });

  it("takes the questions of the page's examples", () => {
    expect(validateSave(prompt(block<Rec[]>("questions")))).toBeUndefined();
  });
});

describe("the audience is a set", () => {
  it("counts a role once and sorts by bytes", () => {
    expect(audienceSet(["b", "a", "b", "B"])).toEqual(["B", "a", "b"]);
  });
});

// ---- the vectors, through the route function and through HTTP --------------------------------------------

type Out = { status: number; body: Rec | undefined; headers: Headers };
type Who = "sharer" | "plain";
type Driver = {
  name: string;
  person: (who: Who) => Promise<unknown>;
  call: (
    person: unknown,
    r: { method: string; path: string; headers?: Record<string, string>; body?: string },
  ) => Promise<Out>;
};

const server = createMockServer({ stepMs: 5, keepaliveMs: 50 });
let base = "";
beforeAll(async () => {
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(async () => {
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});

const ROLES: Record<Who, string[]> = { sharer: ["prompt-sharer", "user"], plain: ["user"] };
let sessions = 0;

const direct: Driver = {
  name: "the route function",
  person: async (who) => ({ who, shelf: newShelf() as Shelf }),
  call: async (p, r) => {
    const { who, shelf } = p as { who: Who; shelf: Shelf };
    const url = new URL(r.path, "http://x");
    const headers = r.headers ?? { "content-type": "application/json" };
    const answered = handlePrompts(
      shelf,
      { roles: ROLES[who], canShare: who === "sharer" },
      {
        method: r.method,
        rest: url.pathname.slice("/api/prompts".length),
        search: url.searchParams,
        contentType: headers["content-type"],
        body: r.body ?? "",
      },
      new Date(),
    );
    if (!answered) throw new Error(`no route for ${r.method} ${r.path}`);
    return {
      status: answered.status,
      body: typeof answered.body === "object" ? (answered.body as Rec) : undefined,
      headers: new Headers({ "content-type": answered.type ?? "application/json" }),
    };
  },
};

const http: Driver = {
  name: "HTTP",
  person: async (who) => {
    const session = `prompts-${++sessions}-${who}`;
    const me = who === "sharer" ? "prompt-sharer" : "user";
    const res = await fetch(`${base}/__mock/config?me=${me}&session=${session}`, {
      method: "POST",
    });
    expect(res.status).toBe(204);
    return session;
  },
  call: async (session, r) => {
    const res = await fetch(base + r.path, {
      method: r.method,
      headers: {
        Cookie: `mock-registry=${session as string}`,
        ...(r.headers ?? { "content-type": "application/json" }),
      },
      ...(r.body === undefined ? {} : { body: r.body }),
    });
    const text = await res.text();
    return {
      status: res.status,
      body: text === "" ? undefined : (JSON.parse(text) as Rec),
      headers: res.headers,
    };
  },
};

const json = (o: unknown): string => JSON.stringify(o);

describe.each([direct, http])("the vectors through $name", (driver) => {
  describe("a whole save (prompts.json)", () => {
    const file = read<{
      cases: {
        name: string;
        create: Rec;
        expect: "ok" | { status: number; code: string; at?: string; offset?: number };
      }[];
    }>("prompts.json");
    it("has the cases the page names", () => {
      expect(file.cases.length).toBeGreaterThan(100);
    });
    for (const c of file.cases) {
      it(c.name, async () => {
        const person = await driver.person("sharer");
        const res = await driver.call(person, {
          method: "POST",
          path: "/api/prompts",
          body: json(expand(c.create)),
        });
        if (c.expect === "ok") {
          expect(res.status, JSON.stringify(res.body)).toBe(201);
          expect(res.body?.id).toMatch(/^p-[a-z0-9]{12}$/);
          return;
        }
        expect(res.status, JSON.stringify(res.body)).toBe(c.expect.status);
        expect(res.body?.code).toBe(c.expect.code);
        if (c.expect.at !== undefined) expect(res.body?.at).toBe(c.expect.at);
        else expect(res.body?.at).toBeUndefined();
        if (c.expect.offset !== undefined) expect(res.body?.offset).toBe(c.expect.offset);
        else expect(res.body?.offset).toBeUndefined();
      });
    }
  });

  describe("render (render.json)", () => {
    const file = read<{
      cases: {
        name: string;
        prompt: { body: Json; variables: Json[] };
        request: Rec;
        expect: { text: Json } | { question: string; rule: string } | { code: string };
      }[];
    }>("render.json");
    it("has the cases the page names", () => {
      expect(file.cases.length).toBeGreaterThan(70);
    });
    for (const c of file.cases) {
      it(c.name, async () => {
        const person = await driver.person("sharer");
        const made = await driver.call(person, {
          method: "POST",
          path: "/api/prompts",
          body: json(expand({ name: "p", title: "T", ...c.prompt })),
        });
        expect(made.status, JSON.stringify(made.body)).toBe(201);
        const res = await driver.call(person, {
          method: "POST",
          path: `/api/prompts/${made.body?.id as string}/render`,
          body: json(expand(c.request)),
        });
        const want = expand(c.expect) as Rec;
        if ("text" in want) {
          expect(res.status, JSON.stringify(res.body)).toBe(200);
          expect(res.body).toEqual({ text: want.text, revision: made.body?.revision });
        } else if ("rule" in want) {
          expect(res.status, JSON.stringify(res.body)).toBe(422);
          expect(res.body).toMatchObject({
            code: "invalid_answer",
            question: want.question,
            rule: want.rule,
          });
        } else {
          expect(res.status).toBe(422);
          expect(res.body?.code).toBe(want.code);
        }
      });
    }
  });

  describe("a request (requests.json)", () => {
    const file = read<{
      seed: Rec[];
      cases: {
        name: string;
        as?: Who;
        method: string;
        path: string;
        headers?: Record<string, string>;
        body?: Json;
        expect: { status: number; code?: string; text?: string; revision?: string };
      }[];
    }>("requests.json");
    it("has the cases the page names", () => {
      expect(file.cases.length).toBeGreaterThan(60);
    });
    // one library for all the cases: none changes a seed
    let sharer: unknown;
    let plain: unknown;
    const ids: string[] = [];
    const revisions: string[] = [];
    const fill = (s: string): string =>
      s.replace(
        /\$(id|rev)(\d)/g,
        (_, kind: string, n: string) => (kind === "id" ? ids : revisions)[Number(n)] ?? "",
      );
    beforeAll(async () => {
      sharer = await driver.person("sharer");
      plain = await driver.person("plain");
      for (const seed of file.seed) {
        const res = await driver.call(sharer, {
          method: "POST",
          path: "/api/prompts",
          body: json(seed),
        });
        expect(res.status, JSON.stringify(res.body)).toBe(201);
        ids.push(res.body?.id as string);
        revisions.push(res.body?.revision as string);
      }
    });
    for (const c of file.cases) {
      it(c.name, async () => {
        const body = c.body === undefined ? undefined : fill(expand(c.body) as string);
        const res = await driver.call(c.as === "plain" ? plain : sharer, {
          method: c.method,
          path: fill(c.path),
          ...(c.headers ? { headers: c.headers } : {}),
          ...(body === undefined ? {} : { body }),
        });
        expect(res.status, JSON.stringify(res.body)).toBe(c.expect.status);
        if (c.expect.code) expect(res.body?.code).toBe(c.expect.code);
        if (c.expect.text) expect(res.body?.text).toBe(c.expect.text);
        if (c.expect.revision) expect(res.body?.revision).toBe(fill(c.expect.revision));
      });
    }
  });
});

// ---- the rules the vectors cannot say: who sees what, which error wins, the size of a page ----------------

describe("the route function keeps the library private (hand-written cases)", () => {
  const J = { "content-type": "application/json" };
  const NOW = new Date("2026-11-15T00:00:00Z");
  type Roles = { roles: string[]; canShare: boolean };
  const call = (
    shelf: Shelf,
    caller: Roles,
    method: string,
    rest: string,
    body?: unknown,
    search = "",
  ) =>
    handlePrompts(
      shelf,
      caller,
      {
        method,
        rest,
        search: new URLSearchParams(search),
        contentType: J["content-type"],
        body: body === undefined ? "" : JSON.stringify(body),
      },
      NOW,
    ) as { status: number; body?: Rec };
  const sharer: Roles = { roles: ["user"], canShare: true };
  const plain: Roles = { roles: ["user"], canShare: false };

  it("get and render of a prompt shared with a role the caller lacks are a 404, as for an id that never existed", () => {
    const shelf = newShelf();
    expect(
      addShared(shelf, [{ name: "t", title: "T", body: "x", audience: ["team"] }], NOW),
    ).toBeUndefined();
    const [id] = [...shelf.others.keys()] as [string];
    const missing = call(shelf, plain, "GET", "/p-zzzzzzzzzzzz");
    for (const answered of [
      call(shelf, plain, "GET", `/${id}`),
      call(shelf, plain, "POST", `/${id}/render`, { answers: [] }),
    ]) {
      expect(answered.status).toBe(404);
      expect(answered.body).toEqual(missing.body);
    }
    // a caller who has the role reads it
    expect(call(shelf, { roles: ["team"], canShare: false }, "GET", `/${id}`).status).toBe(200);
    // roles are compared byte for byte
    expect(call(shelf, { roles: ["Team"], canShare: false }, "GET", `/${id}`).status).toBe(404);
  });

  it("a person who lost `prompts.share` and holds a stale revision is told 403 before 409", () => {
    const shelf = newShelf();
    const made = call(shelf, sharer, "POST", "", {
      name: "s",
      title: "T",
      body: "x",
      audience: ["team"],
    });
    expect(made.status).toBe(201);
    const { id, revision } = made.body as { id: string; revision: string };
    const stale = String(Number(revision) + 99);
    const answered = call(shelf, plain, "PATCH", `/${id}`, { revision: stale, title: "New" });
    expect(answered.status).toBe(403);
    expect(answered.body?.code).toBe("forbidden");
    // with the permission the same request is the stale revision
    expect(call(shelf, sharer, "PATCH", `/${id}`, { revision: stale, title: "New" }).status).toBe(
      409,
    );
  });

  it("a page of exactly 100 has no nextCursor, and a 101st visible prompt gives one", () => {
    const shelf = newShelf();
    for (let i = 0; i < 100; i++) {
      expect(call(shelf, plain, "POST", "", { name: `p${i}`, title: "T", body: "x" }).status).toBe(
        201,
      );
    }
    const full = call(shelf, plain, "GET", "").body as { items: { id: string }[] };
    expect(full.items.length).toBe(100);
    expect(full).not.toHaveProperty("nextCursor");
    addShared(shelf, [{ name: "s", title: "S", body: "x", audience: ["*"] }], NOW);
    const over = call(shelf, plain, "GET", "").body as {
      items: { id: string }[];
      nextCursor?: string;
    };
    expect(over.items.length).toBe(100);
    expect(over.nextCursor).toBe(over.items[99]?.id);
  });
});
