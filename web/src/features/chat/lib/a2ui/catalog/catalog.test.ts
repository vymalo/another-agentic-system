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
  3: "sha256:9f65f9e6ddd424688b1cf61c47634eafee321a3b6736fb7fea8c0f7db1fc7579",
  4: "sha256:20f14ce343579fd7b1f26e15986a05142d037d29ffbd3453e23fbfa07e11fe6f",
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

describe("version 4: Image (ADR 0032)", () => {
  const check = (component: string, instance: unknown) =>
    compileCatalog(OWN_CATALOG.catalog).check(component, instance);
  const SHA = "a1".repeat(32);
  const image = (over: Record<string, unknown> = {}) => ({
    id: "pic",
    component: "Image",
    artifact: SHA,
    alt: "A bar chart of the results",
    ...over,
  });

  it("is in the catalog, which is at least version 4", () => {
    expect(lock.version).toBeGreaterThanOrEqual(4);
    expect(componentNames(OWN_CATALOG.catalog)).toContain("Image");
  });

  it("accepts the smallest instance and one with every property", () => {
    expect(check("Image", image())).toBeUndefined();
    expect(check("Image", image({ caption: "Figure 1", weight: 2 }))).toBeUndefined();
  });

  it("names a file by its sha256 (64 lower-case hex digits) and nothing else", () => {
    for (const bad of [
      "",
      "a1".repeat(31),
      `${"a1".repeat(32)}0`,
      "A1".repeat(32),
      "g".repeat(64),
      `sha256:${SHA}`,
      "https://example.com/a.png",
      "/api/threads/t/artifacts/x",
      "data:image/png;base64,AAAA",
      ` ${SHA}`,
      `${SHA}\n`,
    ]) {
      expect(check("Image", image({ artifact: bad })), JSON.stringify(bad)).toMatch(/^artifact: /);
    }
    expect(check("Image", image({ artifact: { path: "/pic" } }))).toMatch(/^artifact: /);
  });

  it("has no member that could carry a URL: url, src, href and uri are refused", () => {
    for (const key of ["url", "src", "href", "uri", "imageUrl"]) {
      expect(check("Image", image({ [key]: "https://example.com/a.png" })), key).toMatch(
        new RegExp(key),
      );
    }
  });

  it("needs its alt text: 1 to 300 characters; a caption is at most 500", () => {
    const { alt: _alt, ...noAlt } = image();
    expect(check("Image", noAlt)).toMatch(/required property "alt"/);
    expect(check("Image", image({ alt: "" }))).toMatch(/^alt: /);
    expect(check("Image", image({ alt: "a".repeat(300) }))).toBeUndefined();
    expect(check("Image", image({ alt: "a".repeat(301) }))).toMatch(/^alt: /);
    expect(check("Image", image({ caption: "c".repeat(500) }))).toBeUndefined();
    expect(check("Image", image({ caption: "c".repeat(501) }))).toMatch(/^caption: /);
    const { artifact: _artifact, ...noArtifact } = image();
    expect(check("Image", noArtifact)).toMatch(/required property "artifact"/);
  });
});

describe("what the descriptions tell an agent", () => {
  const description = (name: string) =>
    (OWN_CATALOG.catalog.components[name] as { description: string }).description;

  it("Choices is for ask_user only: a surface the agent shows is not answered", () => {
    expect(description("Choices")).toMatch(/^ask_user only/);
  });

  it("Image is for the thread's own files, never a URL, and needs alt", () => {
    expect(description("Image")).toMatch(/file of this thread/);
    expect(description("Image")).toMatch(/never a URL/);
    expect(description("Image")).toMatch(/alt is required/);
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

describe("version 3: Cards and Mermaid", () => {
  const check = (component: string, instance: unknown) =>
    compileCatalog(OWN_CATALOG.catalog).check(component, instance);
  const cards = (over: Record<string, unknown> = {}) => ({
    id: "c",
    component: "Cards",
    cards: [{ title: "A" }],
    ...over,
  });
  const mermaid = (over: Record<string, unknown> = {}) => ({
    id: "m",
    component: "Mermaid",
    code: "graph TD; A-->B",
    ...over,
  });

  it("are in the catalog, which is at least version 3", () => {
    expect(lock.version).toBeGreaterThanOrEqual(3);
    expect(componentNames(OWN_CATALOG.catalog)).toEqual(
      expect.arrayContaining(["Text", "Column", "Choices", "Cards", "Mermaid"]),
    );
  });

  it("accept an instance with every property, and the smallest one", () => {
    expect(
      check(
        "Cards",
        cards({
          title: "T",
          layout: "grid",
          weight: 1,
          cards: [
            {
              title: "A",
              subtitle: "s",
              body: "b",
              url: "https://example.com/a",
              tags: ["x", "y"],
            },
          ],
        }),
      ),
    ).toBeUndefined();
    expect(check("Cards", cards())).toBeUndefined();
    expect(check("Mermaid", mermaid({ title: "T", caption: "C", weight: 2 }))).toBeUndefined();
    expect(check("Mermaid", mermaid())).toBeUndefined();
  });

  it("Cards: the limits are exact, and the reason names the place", () => {
    const n = (count: number) => Array.from({ length: count }, (_, i) => ({ title: `c${i}` }));
    expect(check("Cards", cards({ cards: n(24) }))).toBeUndefined();
    expect(check("Cards", cards({ cards: n(25) }))).toMatch(/^cards: /);
    expect(check("Cards", cards({ cards: [] }))).toMatch(/^cards: /);
    expect(check("Cards", cards({ cards: [{ subtitle: "x" }] }))).toMatch(/title/);
    expect(check("Cards", cards({ cards: [{ title: "" }] }))).toMatch(/^cards\.0\.title: /);
    expect(check("Cards", cards({ cards: [{ title: "t".repeat(201) }] }))).toMatch(
      /^cards\.0\.title: /,
    );
    expect(check("Cards", cards({ cards: [{ title: "a", body: "b".repeat(2001) }] }))).toMatch(
      /^cards\.0\.body: /,
    );
    expect(check("Cards", cards({ cards: [{ title: "a", tags: Array(9).fill("x") }] }))).toMatch(
      /^cards\.0\.tags: /,
    );
    expect(check("Cards", cards({ cards: [{ title: "a", image: "x" }] }))).toMatch(/image/);
    expect(check("Cards", cards({ title: "t".repeat(121) }))).toMatch(/^title: /);
    expect(check("Cards", cards({ layout: "masonry" }))).toMatch(/^layout: /);
  });

  it("Cards: a link starts with http:// or https://, and is at most 2048 characters", () => {
    const url = (u: string) => check("Cards", cards({ cards: [{ title: "a", url: u }] }));
    expect(url("https://example.com/")).toBeUndefined();
    expect(url("http://example.com/")).toBeUndefined();
    expect(url(`https://e.example/${"p".repeat(2048 - 18)}`)).toBeUndefined();
    expect(url(`https://e.example/${"p".repeat(2048 - 17)}`)).toMatch(/^cards\.0\.url: /);
    for (const bad of [
      "javascript:alert(1)",
      "data:text/html,x",
      "/relative",
      "ftp://x",
      "HTTPS://x",
    ]) {
      expect(url(bad), bad).toMatch(/^cards\.0\.url: /);
    }
  });

  it("Mermaid: 1 to 20,000 characters of source, a title of 120 and a caption of 500", () => {
    expect(check("Mermaid", mermaid({ code: "g".repeat(20_000) }))).toBeUndefined();
    expect(check("Mermaid", mermaid({ code: "g".repeat(20_001) }))).toMatch(/^code: /);
    expect(check("Mermaid", mermaid({ code: "" }))).toMatch(/^code: /);
    expect(check("Mermaid", { id: "m", component: "Mermaid" })).toMatch(/required property "code"/);
    expect(check("Mermaid", mermaid({ title: "t".repeat(121) }))).toMatch(/^title: /);
    expect(check("Mermaid", mermaid({ caption: "c".repeat(501) }))).toMatch(/^caption: /);
    expect(check("Mermaid", mermaid({ theme: "dark" }))).toMatch(/theme/);
  });

  it("neither takes a binding or a function call where a literal is required", () => {
    expect(check("Mermaid", mermaid({ code: { path: "/code" } }))).toMatch(/^code: /);
    expect(check("Cards", cards({ cards: { path: "/cards" } }))).toMatch(/^cards: /);
    expect(check("Cards", cards({ title: { call: "formatString", args: {} } }))).toMatch(
      /^title: /,
    );
  });
});
