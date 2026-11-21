import v7 from "./catalog.v7.json";
import type { UiCatalog } from "./index";

/**
 * Test support: the catalog of the version before, rebuilt by taking away what a later version added. A
 * version only adds, so what is left must be the earlier catalog **digest for digest**; that is the proof
 * that an agent and a client that know only the earlier version keep working (every Choices they can write
 * is still valid, and a later member is refused where the earlier schema says).
 *
 * `catalog.v7.json` is the catalog of version 7 as it was released (a copy of the file at the commit that had it, whose
 * digest is `V7_DIGEST`): the descriptions the additions lengthened are taken from it, not typed here, so the
 * proof covers the words too.
 */

type Rec = Record<string, unknown>;

/** The version 7 catalog as released: a v1 document, so it has no `protocolVersion`. */
export const VERSION_7 = v7 as unknown as Omit<UiCatalog, "protocolVersion">;

/** The digest version 7 shipped with. */
export const V7_DIGEST = "sha256:6df9a37d0ae6fa53b8d22145dc1b5674b6d3f228d1c1b83b2ec640d7b3093d49";

/** The catalog of version 7: `catalog` without `protocolVersion`, `show`, `dropped` and `note`, and with the old descriptions. */
export function asVersion7(catalog: UiCatalog): Omit<UiCatalog, "protocolVersion"> {
  const copy = structuredClone(catalog) as unknown as {
    protocolVersion?: string;
    components: Record<string, Rec>;
  };
  // version 7 is a v1 document: the v2 envelope's `protocolVersion` is not part of it
  delete copy.protocolVersion;
  const released = (v7 as unknown as { components: Record<string, Rec> }).components;
  const choices = copy.components.Choices as Rec & { properties: Rec };
  choices.description = released.Choices?.description;
  const question = (choices.properties.questions as Rec).items as { properties: Rec };
  delete question.properties.show;
  const checklist = copy.components.Checklist as Rec & { properties: Rec };
  checklist.description = released.Checklist?.description;
  const item = (checklist.properties.items as Rec).items as { properties: Rec };
  delete item.properties.note;
  (item.properties.state as { enum: string[] }).enum = ["done", "doing", "todo", "failed"];
  return copy as unknown as Omit<UiCatalog, "protocolVersion">;
}
