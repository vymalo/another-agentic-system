import { Validator } from "@cfworker/json-schema";
import { type ComponentSchema, OWN_CATALOG, type UiCatalog } from "./index";

/**
 * The component schemas of a catalog, compiled once. `@cfworker/json-schema` interprets a schema
 * (it does not generate code, so a page that forbids `eval` is fine) and reads draft 2020-12.
 * Our schemas are self-contained: no `$ref`, no `unevaluated*` (docs/api/ui-catalog-v2.md).
 * A catalog of an earlier version (no `protocolVersion`) compiles the same: only its components matter.
 */
export type CompiledCatalog = {
  catalogId: string;
  has: (component: string) => boolean;
  /** `undefined` when the instance fits its component's schema, else why not (the first rule it breaks). */
  check: (component: string, instance: unknown) => string | undefined;
};

/** What `compileCatalog` needs of a catalog: a v1 document and a v2 one both have it. */
type Compilable = { catalogId: string; components: Record<string, ComponentSchema> };

// biome-ignore lint/suspicious/noControlCharactersInRegex: stripping them is the point
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/g;

/** The error of the deepest rule: with `shortCircuit` the list is the chain from the root to it. */
function describe(errors: readonly { instanceLocation: string; error: string }[]): string {
  const last = errors[errors.length - 1];
  if (!last) return "does not fit its schema";
  const where = last.instanceLocation.replace(/^#\/?/, "").split("/").filter(Boolean).join(".");
  const what = last.error.replace(CONTROL, "?").slice(0, 160);
  return where ? `${where}: ${what}` : what;
}

export function compileCatalog(catalog: Compilable): CompiledCatalog {
  const validators = new Map<string, Validator>();
  for (const [name, schema] of Object.entries(catalog.components)) {
    validators.set(name, new Validator(schema, "2020-12", true));
  }
  return {
    catalogId: catalog.catalogId,
    has: (component) => validators.has(component),
    check: (component, instance) => {
      const validator = validators.get(component);
      if (!validator) return "is not in the catalog";
      const result = validator.validate(instance);
      return result.valid ? undefined : describe(result.errors);
    },
  };
}

const compiled = new WeakMap<UiCatalog, CompiledCatalog>();

/** `compileCatalog`, once per catalog object. */
export function compiledOf(catalog: UiCatalog): CompiledCatalog {
  let hit = compiled.get(catalog);
  if (!hit) {
    hit = compileCatalog(catalog);
    compiled.set(catalog, hit);
  }
  return hit;
}

/** The catalog of this build, compiled. */
export const OWN_COMPILED: CompiledCatalog = compiledOf(OWN_CATALOG.catalog);
