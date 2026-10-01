import { safeHttpUrl } from "./url";

/**
 * The `Cards` component of the UI catalog (docs/api/ui-catalog-v1.md, version 3): a list or a grid
 * of up to 24 cards, each with a title, an optional subtitle and body, tags and a link.
 *
 * Pure: what a Cards says, and what is wrong with its links. The JSON Schema in `catalog/catalog.json`
 * is what makes an instance valid; this module reads one that passed it. A card never loads
 * anything (ADR 0013 rule 5): there is no image, no favicon, no preview; a link is a link the
 * person may follow, drawn with the host it goes to.
 */

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

export type CardSpec = {
  title: string;
  subtitle?: string;
  body?: string;
  /** The link as it is drawn: the normalised `href` of a URL that passed `safeHttpUrl`. */
  href?: string;
  /** Where the link goes, for the person to see before they follow it. */
  host?: string;
  tags: string[];
};

export type CardsSpec = {
  title?: string;
  layout: "list" | "grid";
  cards: CardSpec[];
};

/** The host of a link that `safeHttpUrl` accepted, without a leading `www.`. */
export function hostOf(href: string): string {
  try {
    return new URL(href).host.replace(/^www\./i, "");
  } catch {
    return href;
  }
}

function readCard(v: unknown): CardSpec | undefined {
  if (!isRecord(v)) return undefined;
  const title = str(v.title);
  if (title === undefined) return undefined;
  const subtitle = str(v.subtitle);
  const body = str(v.body);
  // the renderer checks the link again: one that does not pass is not a link (the validator has
  // refused the surface already; this is the second check of ADR 0013 rule 6)
  const href = safeHttpUrl(v.url);
  const tags = Array.isArray(v.tags)
    ? v.tags.filter((t): t is string => typeof t === "string")
    : [];
  return {
    title,
    ...(subtitle ? { subtitle } : {}),
    ...(body ? { body } : {}),
    ...(href ? { href, host: hostOf(href) } : {}),
    tags,
  };
}

/** The Cards a component object says, or undefined when it is not one. */
export function readCards(component: Rec): CardsSpec | undefined {
  if (!Array.isArray(component.cards)) return undefined;
  const cards = component.cards.map(readCard);
  if (cards.length === 0 || cards.some((c) => c === undefined)) return undefined;
  const title = str(component.title);
  return {
    ...(title ? { title } : {}),
    layout: component.layout === "grid" ? "grid" : "list",
    cards: cards as CardSpec[],
  };
}

/**
 * The first card whose `url` is not an absolute http(s) URL (`javascript:`, `data:`, a relative
 * path, user information, a control character): its position, for a refusal's reason. The schema
 * only says a URL starts with `http://` or `https://`; `safeHttpUrl` is the rule of ADR 0013.
 */
export function firstBadUrl(component: Rec): number | undefined {
  if (!Array.isArray(component.cards)) return undefined;
  for (const [i, card] of component.cards.entries()) {
    if (isRecord(card) && card.url !== undefined && safeHttpUrl(card.url) === undefined) return i;
  }
  return undefined;
}
