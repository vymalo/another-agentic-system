/**
 * The model and the reasoning effort a person picks for a message or a thread (ADR 0086,
 * `model-pick/v1` and adam's `reasoning/v1`, [`docs/api/model-pick-v1.md`](../../../../../docs/api/model-pick-v1.md)):
 * what the agent's live card offers, read from `AgentInfo.pick` of `GET /api/agents`, and what a run
 * sends in `forwardedProps`. Pure, so the chip and the tests say the same thing.
 *
 * The listing is a hint for the screen, never a check: the orchestrator refuses a pick the live card
 * does not offer (422 with `offered`), and the message wins over the thread, which wins over the
 * agent's own default.
 */

/** The A2A extension an agent lists when it accepts a model pick (ADR 0086). */
export const MODEL_PICK_URI = "https://agents.vymalo.com/a2a/extensions/model-pick/v1";

/** adam's own extension for the reasoning effort, used as it stands. */
export const REASONING_URI = "https://agents.vymalo.com/a2a/extensions/reasoning/v1";

/** `forwardedProps["vymalo.model"]` of a run that carries a message: a model alias. */
export const MODEL_PROP = "vymalo.model";

/** `forwardedProps["vymalo.reasoningEffort"]` of the same run: one of the seven levels. */
export const REASONING_PROP = "vymalo.reasoningEffort";

/** The reasoning levels, lowest first, as the contract spells them. */
export const EFFORTS = ["none", "minimal", "low", "medium", "high", "xhigh", "max"] as const;
export type Effort = (typeof EFFORTS)[number];

/** A word that is one of the seven, spelled exactly; anything else is no level. */
export const parseEffort = (value: unknown): Effort | undefined =>
  typeof value === "string" ? EFFORTS.find((e) => e === value) : undefined;

/** One model alias the card offers, with the label a screen shows. */
export type ModelChoice = { id: string; label: string };

/**
 * What an agent's card lists for a pick, as `GET /api/agents` carries it under `pick`. An axis the
 * card does not list is empty, and a screen that finds neither draws no picker.
 */
export type PickListing = {
  models: readonly ModelChoice[];
  /** The agent's own default alias, when the card names one. */
  modelDefault?: string;
  efforts: readonly Effort[];
  /** The agent's own default level, when the card names one. */
  effortDefault?: Effort;
};

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const nonEmpty = (value: unknown): value is string => typeof value === "string" && value.length > 0;

/**
 * `AgentInfo.pick`, read as strictly as the contract has it: `models` a list of objects with a
 * non-empty `id` and a `label`, each alias at most once, `modelDefault` one of them; `efforts` a
 * list of the seven words, each at most once, `effortDefault` one of them. Anything else is no
 * listing at all, so no chip: the orchestrator never puts a malformed one there, and a client that
 * got one fails closed.
 */
export function parseListing(value: unknown): PickListing | null {
  if (value === undefined || value === null) return null;
  if (!isObject(value)) return null;
  const { models, modelDefault, efforts, effortDefault } = value;

  const parsedModels: ModelChoice[] = [];
  if (models !== undefined) {
    if (!Array.isArray(models)) return null;
    for (const item of models) {
      if (!isObject(item) || !nonEmpty(item.id) || typeof item.label !== "string") return null;
      if (parsedModels.some((m) => m.id === item.id)) return null;
      parsedModels.push({ id: item.id, label: item.label });
    }
  }
  let defModel: string | undefined;
  if (modelDefault !== undefined) {
    if (typeof modelDefault !== "string") return null;
    if (!parsedModels.some((m) => m.id === modelDefault)) return null;
    defModel = modelDefault;
  }

  const parsedEfforts: Effort[] = [];
  if (efforts !== undefined) {
    if (!Array.isArray(efforts)) return null;
    for (const word of efforts) {
      const level = parseEffort(word);
      if (level === undefined || parsedEfforts.includes(level)) return null;
      parsedEfforts.push(level);
    }
  }
  const defEffort = effortDefault === undefined ? undefined : parseEffort(effortDefault);
  if (effortDefault !== undefined && defEffort === undefined) return null;
  if (defEffort !== undefined && !parsedEfforts.includes(defEffort)) return null;

  const listing: PickListing = {
    models: parsedModels,
    efforts: EFFORTS.filter((e) => parsedEfforts.includes(e)),
  };
  if (defModel !== undefined) listing.modelDefault = defModel;
  if (defEffort !== undefined) listing.effortDefault = defEffort;
  return listing;
}

/** Whether the card offers any model alias at all, so a chip may be drawn. */
export const offersModels = (listing: PickListing): boolean => listing.models.length > 0;

/** Whether the card lists any reasoning level, so a chip may be drawn. */
export const offersEfforts = (listing: PickListing): boolean => listing.efforts.length > 0;

/**
 * What the pick axes show. The person's own choice when they made one and the card still lists it;
 * otherwise the agent's default, marked as not picked (nothing is sent: the agent's own default
 * applies). `undefined` on an axis means the card lists nothing there, so nothing is drawn for it.
 */
export type ShownPick = {
  model?: { id: string; label: string; picked: boolean };
  effort?: { level: Effort; picked: boolean };
};

/** What the chips show for `choice`, against what the card lists now. */
export function shownPick(
  listing: PickListing,
  choice: { model?: string; effort?: Effort },
): ShownPick {
  const shown: ShownPick = {};

  if (listing.models.length > 0) {
    const pickedModel =
      choice.model !== undefined ? listing.models.find((m) => m.id === choice.model) : undefined;
    if (pickedModel !== undefined) {
      shown.model = { id: pickedModel.id, label: pickedModel.label, picked: true };
    } else if (listing.modelDefault !== undefined) {
      const rest = listing.models.find((m) => m.id === listing.modelDefault);
      if (rest !== undefined) shown.model = { id: rest.id, label: rest.label, picked: false };
    }
  }

  if (listing.efforts.length > 0) {
    if (choice.effort !== undefined && listing.efforts.includes(choice.effort)) {
      shown.effort = { level: choice.effort, picked: true };
    } else if (listing.effortDefault !== undefined) {
      shown.effort = { level: listing.effortDefault, picked: false };
    }
  }

  return shown;
}

/**
 * The `forwardedProps` a run carries for `choice`: only the axes the person actually picked, as the
 * contract has it (`vymalo.model`, `vymalo.reasoningEffort`). An axis left alone sends nothing, so
 * the agent's own default applies rather than a value the person never chose.
 */
export function pickProps(choice: { model?: string; effort?: Effort }): Record<string, string> {
  const props: Record<string, string> = {};
  if (choice.model !== undefined && choice.model.length > 0) props[MODEL_PROP] = choice.model;
  if (choice.effort !== undefined) props[REASONING_PROP] = choice.effort;
  return props;
}
