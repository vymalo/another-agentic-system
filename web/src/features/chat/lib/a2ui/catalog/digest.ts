/**
 * The digest of a UI catalog (docs/api/ui-catalog-v1.md, "Digest"):
 * `"sha256:" + lowercase hex(SHA-256(UTF-8(canonical(catalog))))`, where canonical is JSON with the
 * keys of every object sorted, no whitespace, strings escaped as `JSON.stringify` does and integers
 * in decimal: RFC 8785 restricted to what a catalog holds. A key that is not ASCII (so that code
 * point order and UTF-16 order cannot differ) and a number that is not an integer are refused
 * rather than canonicalised. The orchestrator recomputes the digest in Rust and must get the same
 * string; the known-answer vector in `catalog.test.ts` pins both ends.
 *
 * The app itself ships the digest of `catalog.lock.json` and never computes one: this is for the
 * lock script and for the test that keeps the lock honest.
 */

const isAscii = (s: string): boolean => {
  for (let i = 0; i < s.length; i++) if (s.charCodeAt(i) > 0x7f) return false;
  return true;
};

export class CanonicalError extends Error {}

/** `value` as canonical JSON. Throws `CanonicalError` for what a catalog may not contain. */
export function canonicalJson(value: unknown): string {
  if (value === null) return "null";
  switch (typeof value) {
    case "boolean":
      return value ? "true" : "false";
    case "string":
      return JSON.stringify(value);
    case "number":
      if (!Number.isSafeInteger(value)) {
        throw new CanonicalError(`${String(value)} is not an integer`);
      }
      return String(value);
    case "object": {
      if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
      const keys = Object.keys(value);
      for (const key of keys) {
        if (!isAscii(key)) throw new CanonicalError(`the key ${JSON.stringify(key)} is not ASCII`);
      }
      keys.sort();
      const rec = value as Record<string, unknown>;
      return `{${keys.map((k) => `${JSON.stringify(k)}:${canonicalJson(rec[k])}`).join(",")}}`;
    }
    default:
      throw new CanonicalError(`a ${typeof value} cannot be in a catalog`);
  }
}

/** `sha256:<64 lowercase hex digits>` of the canonical form of `catalog`. */
export async function catalogDigest(catalog: unknown): Promise<string> {
  const bytes = new TextEncoder().encode(canonicalJson(catalog));
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return `sha256:${Array.from(hash, (b) => b.toString(16).padStart(2, "0")).join("")}`;
}
