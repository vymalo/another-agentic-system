import type { UiCatalog } from "./index";

/**
 * Test support: the catalog of the version before, rebuilt by taking away what a later version added. A
 * version only adds, so what is left must be the earlier catalog **digest for digest**; that is the proof
 * that an agent and a client that know only the earlier version keep working (every Choices they can write
 * is still valid, and a later member is refused where the earlier schema says).
 */

type Rec = Record<string, unknown>;

/** The words of version 7's `Choices` description, which version 8 lengthened. */
export const V7_CHOICES_DESCRIPTION =
  'ask_user only: use it in the question you ask the person, never in a surface you show with show (nothing waits for its answer, so its form is dead). A form of up to 8 questions answered with one submit: a title, a description of why you ask, and per question a help line and a type: single (radio buttons, the default), multiple (checkboxes), select (a searchable list, up to 50 options), images (a deck of picture tiles, up to 16, each option\'s image an image file shared in this conversation), text or longText (the person types). Only when the thread\'s catalog is version 7 or more: confirm (yes or no, with yesLabel, noLabel and a safe default), number (a decimal string, with minValue, maxValue, step and unit), scale (a whole number from "from" to "to", with fromLabel and toLabel), date (a calendar day YYYY-MM-DD, with minDate and maxDate) and rank (the person puts every option in order). An option has a label, a description, an optional image {sha256, alt}, an icon and a recommended badge. freeInput adds an anything-else box. One submit sends one action: name = action.event.name (default "answer"), context = {answers:[{id, values:[option value, or the one yes/no, decimal, whole number or date, or every option value in the person\'s order for rank…], other?, text?}], note?} in question order. Read the answer again before you act on it: an out-of-range number, a date outside its bounds or a rank that is not every option once is refused.';

/** And version 6's and 7's `Checklist` description. */
export const V7_CHECKLIST_DESCRIPTION =
  "A checklist of items, each with its state (done, doing, todo or failed), and a progress bar.";

/** The digest version 7 shipped with. */
export const V7_DIGEST = "sha256:6df9a37d0ae6fa53b8d22145dc1b5674b6d3f228d1c1b83b2ec640d7b3093d49";

/** The catalog of version 7: `catalog` without `show`, `dropped` and `note`, and with the old words. */
export function asVersion7(catalog: UiCatalog): UiCatalog {
  const copy = structuredClone(catalog) as unknown as { components: Record<string, Rec> };
  const choices = copy.components.Choices as Rec & { properties: Rec };
  choices.description = V7_CHOICES_DESCRIPTION;
  const question = (choices.properties.questions as Rec).items as { properties: Rec };
  delete question.properties.show;
  const checklist = copy.components.Checklist as Rec & { properties: Rec };
  checklist.description = V7_CHECKLIST_DESCRIPTION;
  const item = (checklist.properties.items as Rec).items as { properties: Rec };
  delete item.properties.note;
  (item.properties.state as { enum: string[] }).enum = ["done", "doing", "todo", "failed"];
  return copy as unknown as UiCatalog;
}
