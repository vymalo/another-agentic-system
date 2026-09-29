import { FIELD, MAX_CONTEXT_BYTES } from "./limits";

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * The context of an action at the click: every `{"$field": key}` (what the validator made of a
 * binding to an input) becomes the input's current value; a value nobody entered is `null`.
 * Nothing else is evaluated: the rest of the context was fixed when the surface was converted.
 */
export function resolveFields(
  context: unknown,
  values: Readonly<Record<string, unknown>>,
  depth = 0,
): unknown {
  if (depth > 32) return null;
  if (Array.isArray(context)) return context.map((x) => resolveFields(x, values, depth + 1));
  if (!isRecord(context)) return context;
  const keys = Object.keys(context);
  if (keys.length === 1 && keys[0] === FIELD && typeof context[FIELD] === "string") {
    const key = context[FIELD];
    // own keys only: an agent's marker must not read `__proto__` or `constructor`
    return Object.hasOwn(values, key) && values[key] !== undefined ? values[key] : null;
  }
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(context)) {
    Object.defineProperty(out, k, {
      value: resolveFields(v, values, depth + 1),
      enumerable: true,
      writable: true,
      configurable: true,
    });
  }
  return out;
}

/** The serialised size of an action's context, against what the orchestrator accepts. */
export const contextTooLarge = (context: unknown): boolean =>
  new TextEncoder().encode(JSON.stringify(context ?? {})).length > MAX_CONTEXT_BYTES;
