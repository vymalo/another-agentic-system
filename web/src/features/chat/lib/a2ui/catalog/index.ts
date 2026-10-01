import catalogJson from "./catalog.json";
import lock from "./catalog.lock.json";

/**
 * The UI catalog of this build (ADR 0023, docs/api/ui-catalog-v1.md): the components an agent may
 * name in a surface under `catalogId`, each as a JSON Schema of its whole instance, in the shape
 * of an A2UI inline catalog. `catalog.json` is the source; `catalog.lock.json` says which version
 * it is and what its digest is (`pnpm catalog:lock`), and a test keeps the two honest.
 */

/** A JSON Schema (draft 2020-12), as a catalog holds it. */
export type ComponentSchema = Record<string, unknown>;

/** The A2UI inline catalog: exactly what travels to the agents. */
export type UiCatalog = {
  catalogId: string;
  components: Record<string, ComponentSchema>;
};

/** What the web sends in `forwardedProps["vymalo.uiCatalog"]` (docs/api/agui.md, "Inbound"). */
export type OwnCatalog = {
  catalogId: string;
  /** Orders catalogs: the highest version a thread has seen is the newest. Bumped by hand. */
  version: number;
  digest: string;
  catalog: UiCatalog;
};

/** What a thread says about its catalog in `STATE_SNAPSHOT.snapshot.thread.uiCatalog`. */
export type UiCatalogRef = { catalogId: string; version: number; digest: string };

/** `forwardedProps` key of the catalog (an A2UI-style, not a URI one: it is the UI's own). */
export const UI_CATALOG_PROP = "vymalo.uiCatalog";

export const OWN_CATALOG: OwnCatalog = {
  catalogId: catalogJson.catalogId,
  version: lock.version,
  digest: lock.digest,
  catalog: catalogJson as UiCatalog,
};

/** The component names of a catalog. */
export const componentNames = (catalog: UiCatalog): string[] => Object.keys(catalog.components);

/**
 * Whether the run that sends this catalog should carry it, given what the thread has recorded
 * (`STATE_SNAPSHOT.thread.uiCatalog`; undefined for a thread that has none, or does not exist yet):
 * when the thread has none, when ours is newer, or when the version is the same and the digest is
 * not (it should not happen: the lock prevents it). An older UI sends nothing and so cannot move
 * a thread back; a thread that has this very digest needs nothing.
 */
export function shouldSendCatalog(own: OwnCatalog, thread: UiCatalogRef | undefined): boolean {
  if (!thread) return true;
  if (own.version !== thread.version) return own.version > thread.version;
  return own.digest !== thread.digest;
}
