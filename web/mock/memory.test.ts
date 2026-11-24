import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  checkCall,
  type Grant,
  listed,
  MAX_NOTE_BYTES,
  Memory,
  MemoryRefused,
  MemorySettings,
  type MemoryTool,
  type ThreadSwitch,
} from "./memory";

/*
 * The memory-v1 vectors (docs/api/examples/memory/*.json), run against the mock's reading. The Rust
 * core runs the same files (orchestrator/crates/core/tests/memory_vectors.rs) with the same
 * conventions: `holdNotes: n` holds "I deploy with Helm", "I use pnpm", "staging is kestrel", then
 * "filler 4", ...; A's ids are m-a-<i> and B's m-b-<i>.
 */
// biome-ignore lint/suspicious/noExplicitAny: vector files are untyped JSON read by key
type Json = Record<string, any>;

const vectors = (file: string): Json[] =>
  JSON.parse(readFileSync(path.resolve("../docs/api/examples/memory", file), "utf8")).cases;

const SEED = ["I deploy with Helm", "I use pnpm", "staging is kestrel"];
const at = (i: number) => `2026-11-24T09:${String(i).padStart(2, "0")}:00Z`;

function hold(m: Memory, who: string, n: number) {
  for (let i = 1; i <= n; i++)
    m.save({ text: SEED[i - 1] ?? `filler ${i}` }, "person", `m-${who}-${i}`, at(i));
}

function inputOf(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(inputOf);
  if (v && typeof v === "object") {
    const o = v as Json;
    if ("$repeat" in o) return String(o.$repeat).repeat(o.times);
    return Object.fromEntries(Object.entries(o).map(([k, x]) => [k, inputOf(x)]));
  }
  return v;
}

function runCall(m: Memory, call: Json, sw: ThreadSwitch, grant: Grant | undefined) {
  const tool = call.tool as MemoryTool;
  const refusal = checkCall(tool, sw, call.caller === "main", grant);
  if (refusal) return { error: refusal as string };
  const input = inputOf(call.input);
  try {
    if (tool === "memory_read") {
      const r = m.read(input);
      return {
        texts: r.notes.map((n) => n.text),
        count: r.notes.length,
        total: r.total,
        truncated: r.truncated,
        newestFirst: r.notes.every(
          (n, i) => i === 0 || (r.notes[i - 1]?.savedAt ?? "") >= n.savedAt,
        ),
      };
    }
    if (tool === "memory_save") {
      const n = m.save(input, call.agent, `m-new-${m.length + 1}`, "2026-11-24T10:00:00Z");
      return { id: n.id, savedAt: n.savedAt };
    }
    return { forgotten: m.forget(input) };
  } catch (e) {
    if (e instanceof MemoryRefused) return { error: e.code as string };
    throw e;
  }
}

describe("memory-v1 vectors", () => {
  it("tools.json", () => {
    const cases = vectors("tools.json");
    expect(cases.length).toBeGreaterThanOrEqual(10);
    for (const c of cases) {
      const grants = c.grants as Record<string, Grant>;
      const grant = grants[c.call.agent];
      const m = new Memory();
      if (c.holdNotes) hold(m, "a", c.holdNotes);
      const out = runCall(m, c.call, c.switch, grant) as Json;
      const e = c.expect as Json;
      if (e.error) {
        expect(out.error, c.name).toBe(e.error);
      } else {
        expect(out.error, c.name).toBeUndefined();
        for (const [k, v] of Object.entries((e.result ?? {}) as Json))
          if (typeof v === "string" && v.endsWith("…"))
            expect(String(out[k]).startsWith(v.slice(0, -1)), `${c.name}: ${k}`).toBe(true);
          else expect(out[k], `${c.name}: ${k}`).toEqual(v);
        if (e.resultNotes !== undefined) expect(out.count, c.name).toBe(e.resultNotes);
        for (const k of ["total", "truncated"])
          if (e[k] !== undefined) expect(out[k], `${c.name}: ${k}`).toBe(e[k]);
        if (e.order === "newest first") expect(out.newestFirst, c.name).toBe(true);
        if (e.storedImmediately) expect(m.length, c.name).toBe(1);
      }
      if (e.notesAfter !== undefined) expect(m.length, c.name).toBe(e.notesAfter);
      if (e.namesBound !== undefined) {
        expect(e.namesBound).toBe(MAX_NOTE_BYTES);
        expect(JSON.stringify(out)).not.toContain("xxx");
      }
      if (e.listed !== undefined)
        expect(listed(c.call.tool, c.switch, c.call.caller === "main", grant), c.name).toBe(
          e.listed,
        );
    }
  });

  it("isolation.json", () => {
    const cases = vectors("isolation.json");
    expect(cases.length).toBeGreaterThanOrEqual(4);
    for (const c of cases) {
      const people = { a: new Memory(), b: new Memory() };
      for (const who of ["a", "b"]) {
        const m = new Memory();
        const texts = c.setup[who].notes as string[];
        for (let i = 0; i < texts.length; i++)
          m.save({ text: texts[i] }, "person", `m-${who}-${i + 1}`, at(i + 1));
        people[who as "a" | "b"] = m;
      }
      const out = runCall(people[c.call.as as "a" | "b"], c.call, "on", {
        read: true,
        write: true,
      }) as Json;
      const e = c.expect as Json;
      if (e.resultNotes !== undefined) expect(out.count, c.name).toBe(e.resultNotes);
      if (e.texts) expect(out.texts, c.name).toEqual(e.texts);
      for (const k of ["total", "truncated"])
        if (e[k] !== undefined) expect(out[k], `${c.name}: ${k}`).toBe(e[k]);
      for (const [k, v] of Object.entries((e.result ?? {}) as Json))
        expect(out[k], c.name).toEqual(v);
      if (e.aNotesAfter !== undefined) expect(people.a.length, c.name).toBe(e.aNotesAfter);
      if (e.bNotesAfter !== undefined) expect(people.b.length, c.name).toBe(e.bNotesAfter);
    }
  });

  it("settings.json", () => {
    const cases = vectors("settings.json");
    expect(cases.length).toBeGreaterThanOrEqual(6);
    for (const c of cases) {
      const s = new MemorySettings();
      const events: string[] = [];
      let status = 200;
      if (c.call.method === "PUT") {
        try {
          s.apply(c.call.body);
          events.push("memory_switched");
        } catch (e) {
          if (!(e instanceof MemoryRefused)) throw e;
          status = 400;
        }
      }
      const e = c.expect as Json;
      expect(status, c.name).toBe(e.status);
      if (e.events) expect(events, c.name).toEqual(e.events);
      const body = JSON.parse(JSON.stringify(s));
      if (e.body) expect(body, c.name).toEqual(e.body);
      for (const [k, v] of Object.entries((e.agents ?? {}) as Json))
        expect(body.agents[k], `${c.name}: ${k}`).toEqual(v);
      if (e.code === "invalid") expect(status, c.name).toBe(400);
    }
  });
});
