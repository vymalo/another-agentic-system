/**
 * JSON Pointer (RFC 6901) reads, with the semantics of the converter of
 * `@assistant-ui/react-generative-ui` 0.0.21 (which the validator has to predict exactly): `""`
 * and `"/"` are the whole document, a path must start with `/`, `~1` and `~0` decode, an array
 * takes a canonical index only, an object takes its own keys only.
 */
const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

export function resolvePointer(source: unknown, path: string): unknown {
  if (path === "" || path === "/") return source;
  if (!path.startsWith("/")) return undefined;
  const segments = path
    .slice(1)
    .split("/")
    .map((s) => s.replaceAll("~1", "/").replaceAll("~0", "~"));
  let current = source;
  for (const segment of segments) {
    if (Array.isArray(current)) {
      if (!/^(0|[1-9]\d*)$/.test(segment)) return undefined;
      current = current[Number(segment)];
      continue;
    }
    if (!isRecord(current) || !Object.hasOwn(current, segment)) return undefined;
    current = current[segment];
  }
  return current;
}

/** `{ "path": "/x" }` and nothing else: a binding to the data model. */
export const isBinding = (v: unknown): v is { path: string } =>
  isRecord(v) && Object.keys(v).length === 1 && typeof v.path === "string";

/** A literal value, or the value a binding reads from `data`. */
export const resolveValue = (v: unknown, data: unknown): unknown =>
  isBinding(v) ? resolvePointer(data, v.path) : v;
