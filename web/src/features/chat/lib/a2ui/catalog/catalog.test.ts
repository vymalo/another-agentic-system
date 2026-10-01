import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import catalogJson from "./catalog.json";
import lock from "./catalog.lock.json";
import { CanonicalError, canonicalJson, catalogDigest } from "./digest";
import {
  componentNames,
  OWN_CATALOG,
  type OwnCatalog,
  shouldSendCatalog,
  type UiCatalog,
} from "./index";
import { compileCatalog, OWN_COMPILED } from "./validate";

/**
 * Every version of the catalog that has shipped, and its digest. The lock says which one this build
 * is; this table is what remembers the others, so a digest cannot be changed under a version a
 * thread may already have recorded. `pnpm catalog:lock <next version>` rewrites the lock and says
 * to add its pair here.
 */
const RELEASED: Record<number, string> = {
  1: "sha256:38baa8cc271178fd944f7ade5ae1578ba4186f444077d2bd10ed6ef9aa98fdbd",
  2: "sha256:4ed91bcc9db52d5e2262aef2091d2b3eeccbf5bfe51519d7326fdc6641fb7856",
};

describe("the digest", () => {
  // computed 2026-10-01 with Python: json.dumps(sort_keys=True, separators=(",", ":"),
  // ensure_ascii=False), then sha256; the orchestrator's core crate pins the same vector
  const KAT = {
    catalogId: "https://agents.vymalo.com/a2ui/catalogs/test",
    components: {
      Note: {
        type: "object",
        properties: { component: { const: "Note" }, text: { type: "string", maxLength: 10 } },
        required: ["component", "text"],
      },
    },
  };

  it("canonical JSON has sorted keys at every level and no whitespace", () => {
    expect(canonicalJson(KAT)).toBe(
      '{"catalogId":"https://agents.vymalo.com/a2ui/catalogs/test","components":{"Note":{"properties":{"component":{"const":"Note"},"text":{"maxLength":10,"type":"string"}},"required":["component","text"],"type":"object"}}}',
    );
  });

  it("the known-answer vector", async () => {
    expect(await catalogDigest(KAT)).toBe(
      "sha256:a237e931c3a02fc72b214561e3a52d33eaf0e29c9306bbe0ef7b3da238507293",
    );
  });

  it("does not depend on the order the keys were written in", async () => {
    const a = { b: 1, a: { d: [1, 2], c: "x" } };
    const b = { a: { c: "x", d: [1, 2] }, b: 1 };
    expect(canonicalJson(a)).toBe(canonicalJson(b));
    expect(await catalogDigest(a)).toBe(await catalogDigest(b));
  });

  it("escapes strings as JSON.stringify does, and keeps array order", () => {
    expect(canonicalJson({ s: 'a"b\\c\n\u0001é', l: [3, 1, 2], t: true, n: null })).toBe(
      '{"l":[3,1,2],"n":null,"s":"a\\"b\\\\c\\n\\u0001é","t":true}',
    );
  });

  it("refuses what it does not canonicalise: a key that is not ASCII, a fractional number", () => {
    expect(() => canonicalJson({ clé: 1 })).toThrow(CanonicalError);
    expect(() => canonicalJson({ a: 1.5 })).toThrow(CanonicalError);
    expect(() => canonicalJson({ a: Number.NaN })).toThrow(CanonicalError);
    expect(() => canonicalJson({ a: undefined })).toThrow(CanonicalError);
  });

  it("is 'sha256:' and 64 lowercase hex digits", async () => {
    expect(await catalogDigest({})).toMatch(/^sha256:[0-9a-f]{64}$/);
  });
});

describe("the lock", () => {
  it("is the digest of catalog.json (the catalog changed without `pnpm catalog:lock <version>`)", async () => {
    expect(await catalogDigest(catalogJson)).toBe(lock.digest);
  });

  it("is a version of at least 1 that is the pair this table remembers for it", () => {
    expect(Number.isSafeInteger(lock.version) && lock.version >= 1).toBe(true);
    expect(
      RELEASED[lock.version],
      `the version ${lock.version} has no digest in RELEASED: add \`${lock.version}: "${lock.digest}"\``,
    ).toBe(lock.digest);
  });

  it("has every version below it in the table, in the order they were released", () => {
    const versions = Object.keys(RELEASED).map(Number);
    expect(versions).toEqual(Array.from({ length: versions.length }, (_, i) => i + 1));
    expect(new Set(Object.values(RELEASED)).size).toBe(versions.length);
    expect(Math.max(...versions)).toBe(lock.version);
  });

  it("is what the build sends", () => {
    expect(OWN_CATALOG).toMatchObject({
      catalogId: "https://agents.vymalo.com/a2ui/catalogs/chat",
      version: lock.version,
      digest: lock.digest,
    });
    expect(OWN_CATALOG.catalog).toEqual(catalogJson);
    expect(OWN_CATALOG.catalog.catalogId).toBe(OWN_CATALOG.catalogId);
  });
});

/** The rules the orchestrator checks on a catalog it is sent (docs/api/ui-catalog-v1.md). */
describe("the catalog document", () => {
  const FORBIDDEN = ["$ref", "$dynamicRef", "$id", "$anchor", "$schema", "$defs", "$comment"];
  const walk = (node: unknown, visit: (key: string) => void, depth = 0): number => {
    if (Array.isArray(node)) return Math.max(0, ...node.map((n) => walk(n, visit, depth + 1)));
    if (typeof node !== "object" || node === null) return depth;
    let deepest = depth;
    for (const [k, v] of Object.entries(node)) {
      visit(k);
      deepest = Math.max(deepest, walk(v, visit, depth + 1));
    }
    return deepest;
  };

  it("is an A2UI inline catalog and nothing more: catalogId and components", () => {
    expect(Object.keys(OWN_CATALOG.catalog).sort()).toEqual(["catalogId", "components"]);
    expect(OWN_CATALOG.catalogId).toMatch(/^https:\/\/\S+$/);
    expect(new TextEncoder().encode(OWN_CATALOG.catalogId).length).toBeLessThanOrEqual(256);
  });

  it("stays inside the limits: 64 KiB, 64 components, 32 levels, names like Choices", () => {
    const names = componentNames(OWN_CATALOG.catalog);
    expect(names.length).toBeGreaterThanOrEqual(1);
    expect(names.length).toBeLessThanOrEqual(64);
    for (const name of names) expect(name).toMatch(/^[A-Z][A-Za-z0-9]{0,63}$/);
    expect(new TextEncoder().encode(JSON.stringify(OWN_CATALOG.catalog)).length).toBeLessThan(
      64 * 1024,
    );
    expect(walk(OWN_CATALOG.catalog, () => {})).toBeLessThanOrEqual(32);
  });

  it("is self-contained draft 2020-12: no $ref, $id, $anchor, $schema; no unevaluated*", () => {
    const keys: string[] = [];
    walk(OWN_CATALOG.catalog, (k) => keys.push(k));
    for (const key of FORBIDDEN) expect(keys, key).not.toContain(key);
    expect(keys.filter((k) => k.startsWith("unevaluated"))).toEqual([]);
  });

  it("describes each component as an instance with its own name and no unknown property", () => {
    for (const [name, schema] of Object.entries(OWN_CATALOG.catalog.components)) {
      const s = schema as {
        type?: string;
        description?: string;
        properties?: Record<string, unknown>;
        required?: string[];
        additionalProperties?: unknown;
      };
      expect(s.type, name).toBe("object");
      expect(typeof s.description, name).toBe("string");
      expect(s.properties?.component, name).toEqual({ const: name });
      expect(s.properties?.id, name).toEqual({ type: "string", minLength: 1, maxLength: 256 });
      expect(s.required, name).toEqual(expect.arrayContaining(["id", "component"]));
      expect(s.additionalProperties, name).toBe(false);
    }
  });

  it("every schema compiles", () => {
    expect(() => compileCatalog(OWN_CATALOG.catalog)).not.toThrow();
    for (const name of componentNames(OWN_CATALOG.catalog))
      expect(OWN_COMPILED.has(name)).toBe(true);
  });
});

/** The contract page holds each component's schema as a JSON block; the build's must be those. */
describe("the contract (docs/api/ui-catalog-v1.md)", () => {
  const doc = readFileSync(
    path.resolve(import.meta.dirname, "../../../../../../../docs/api/ui-catalog-v1.md"),
    "utf8",
  );
  const blocks = [...doc.matchAll(/```json\n([\s\S]*?)```/g)].flatMap(([, body]) => {
    try {
      return [JSON.parse(body ?? "") as Record<string, unknown>];
    } catch {
      return []; // an example that is not a bare object of components
    }
  });

  it("every component of this build is written in the contract, the same", () => {
    for (const [name, schema] of Object.entries(OWN_CATALOG.catalog.components)) {
      const written = blocks.filter((b) => name in b);
      expect(written.length, `${name} is in one JSON block of the contract`).toBe(1);
      expect(written[0]?.[name], name).toEqual(schema);
    }
  });
});

describe("shouldSendCatalog", () => {
  const own: OwnCatalog = { ...OWN_CATALOG, version: 4, digest: `sha256:${"4".repeat(64)}` };
  const thread = (version: number, digest = own.digest) => ({
    catalogId: own.catalogId,
    version,
    digest,
  });

  it("sends to a thread that has none", () => {
    expect(shouldSendCatalog(own, undefined)).toBe(true);
  });
  it("sends when ours is newer", () => {
    expect(shouldSendCatalog(own, thread(3, `sha256:${"3".repeat(64)}`))).toBe(true);
  });
  it("does not send when the thread has the same digest", () => {
    expect(shouldSendCatalog(own, thread(4))).toBe(false);
  });
  it("sends when the version is the same and the digest differs (later wins)", () => {
    expect(shouldSendCatalog(own, thread(4, `sha256:${"a".repeat(64)}`))).toBe(true);
  });
  it("does not send from an older build, whatever the digest", () => {
    expect(shouldSendCatalog(own, thread(5, `sha256:${"5".repeat(64)}`))).toBe(false);
    expect(shouldSendCatalog(own, thread(5))).toBe(false);
  });
});

describe("validating an instance", () => {
  const catalog: UiCatalog = OWN_CATALOG.catalog;
  const check = (component: string, instance: unknown) =>
    compileCatalog(catalog).check(component, instance);

  it("accepts a Text and a Column", () => {
    expect(
      check("Text", { id: "t", component: "Text", text: "hi", variant: "h1" }),
    ).toBeUndefined();
    expect(
      check("Column", { id: "c", component: "Column", children: ["t"], align: "stretch" }),
    ).toBeUndefined();
  });

  it("names the property and the rule it breaks", () => {
    expect(check("Text", { id: "t", component: "Text", text: "x".repeat(4001) })).toMatch(
      /^text: String is too long/,
    );
    expect(check("Text", { id: "t", component: "Text", text: "" })).toMatch(/^text: /);
    expect(check("Text", { id: "t", component: "Text" })).toMatch(/required property "text"/);
    expect(check("Text", { id: "t", component: "Text", text: "a", variant: "h9" })).toMatch(
      /^variant: /,
    );
    expect(check("Text", { id: "t", component: "Text", text: "a", extra: 1 })).toMatch(/extra/);
    expect(check("Column", { id: "c", component: "Column", children: [] })).toMatch(/^children: /);
    expect(check("Column", { id: "c", component: "Column", children: [1] })).toMatch(
      /^children\.0: /,
    );
  });

  it("a Text bound to the data model is not a literal: refused", () => {
    expect(check("Text", { id: "t", component: "Text", text: { path: "/x" } })).toMatch(/^text: /);
  });

  it("the limits are exact: 4000 characters and 50 children are in, one more is out", () => {
    expect(check("Text", { id: "t", component: "Text", text: "x".repeat(4000) })).toBeUndefined();
    const kids = (n: number) => Array.from({ length: n }, (_, i) => `k${i}`);
    expect(check("Column", { id: "c", component: "Column", children: kids(50) })).toBeUndefined();
    expect(check("Column", { id: "c", component: "Column", children: kids(51) })).toMatch(
      /^children: /,
    );
  });

  it("a component that is not in the catalog is said so", () => {
    expect(check("Gizmo", {})).toBe("is not in the catalog");
  });

  it("the name in the instance must be the key's", () => {
    expect(check("Text", { id: "t", component: "Column", text: "hi" })).toMatch(/component/);
  });
});
