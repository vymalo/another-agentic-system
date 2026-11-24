import { useCallback, useEffect, useMemo, useState } from "react";
import {
  type Effort,
  offersEfforts,
  offersModels,
  type PickListing,
  parseListing,
  type ShownPick,
  shownPick,
} from "../lib/pick";

/** What a person picked for the messages of the thread: either axis, or both. */
export type PickChoice = { model?: string; effort?: Effort };

/** What the composer's pick chips need (ADR 0086, `model-pick/v1` and `reasoning/v1`). */
export type PickView = {
  /**
   * What the agent's card lists for a pick, as `GET /api/agents` carries it under `pick`; null when
   * it lists nothing usable, so no chip is drawn.
   */
  listing: PickListing | null;
  /** What the chips show; an axis absent is an axis with no chip. */
  shown: ShownPick | null;
  /**
   * The pick the next message is sent with: only the axes the card lists, and only what the person
   * actually picked (nothing on an axis left alone, so the agent's own default applies).
   */
  send: PickChoice;
  /** The person picks a model or an effort; an axis they do not name keeps what it had. */
  pick: (choice: PickChoice) => void;
};

/**
 * The model and the reasoning effort of the composer (ADR 0086, `docs/api/model-pick-v1.md`).
 *
 * `AgentInfo.pick` is read from the live card on every `GET /api/agents` (ADR 0008: never cached),
 * so an axis it leaves out is an axis the card does not list, or lists badly, and no chip is drawn
 * for it. Nothing is sent until the person picks; a pick is sent with each message of the thread
 * from then on, and the message's pick wins over the thread's default at the orchestrator. A choice
 * the card no longer offers is dropped: the chip shows the agent's default and says it is the
 * default, never a value the agent would refuse.
 *
 * The listing is a hint for the screen, never a check: the orchestrator reads the card itself and
 * answers a pick it does not offer with a typed `422` carrying what it does. The capabilities
 * document is not where the chips come from: it lists the orchestrator's own known extensions and
 * carries no `model-pick/v1` or `reasoning/v1` key.
 *
 * A card that lists choices but names no default still draws its chip: it shows the explicit "Agent
 * default" entry, the menu's first entry, and nothing is sent until the person picks (adam's
 * `model-pick/v1` card lists only `choices` today, ANR-139).
 */
export function usePick({ listing }: { listing: unknown }): PickView {
  const [choice, setChoice] = useState<PickChoice>({});

  const parsed = useMemo(() => parseListing(listing), [listing]);
  const modelOffered = parsed !== null && offersModels(parsed);
  const effortOffered = parsed !== null && offersEfforts(parsed);

  // A pick the card no longer offers is forgotten, and the chip falls back to the agent's default.
  useEffect(() => {
    setChoice((current) => {
      const model =
        current.model !== undefined && parsed?.models.some((choice) => choice.id === current.model)
          ? current.model
          : undefined;
      const effort =
        current.effort !== undefined && parsed?.efforts.includes(current.effort)
          ? current.effort
          : undefined;
      if (model === current.model && effort === current.effort) return current;
      const next: PickChoice = {};
      if (model !== undefined) next.model = model;
      if (effort !== undefined) next.effort = effort;
      return next;
    });
  }, [parsed]);

  const pick = useCallback((next: PickChoice) => {
    setChoice((current) => ({ ...current, ...next }));
  }, []);

  // Only what the card offers and the person picked: an axis left alone sends nothing, so the
  // agent's own default applies rather than a value the person never chose.
  const send = useMemo<PickChoice>(() => {
    const next: PickChoice = {};
    if (
      parsed !== null &&
      choice.model !== undefined &&
      parsed.models.some((offered) => offered.id === choice.model)
    ) {
      next.model = choice.model;
    }
    if (parsed !== null && choice.effort !== undefined && parsed.efforts.includes(choice.effort)) {
      next.effort = choice.effort;
    }
    return next;
  }, [choice, parsed]);

  const shown = useMemo<ShownPick | null>(() => {
    if (parsed === null) return null;
    const all = shownPick(parsed, choice);
    const visible: ShownPick = {};
    if (modelOffered && all.model !== undefined) visible.model = all.model;
    if (effortOffered && all.effort !== undefined) visible.effort = all.effort;
    return visible;
  }, [parsed, choice, modelOffered, effortOffered]);

  return { listing: parsed, shown, send, pick };
}
