/*
 * What the person opened in the panel's step tree. It lives above the panel (the shell holds it,
 * `useStepsExpansion`), so closing the panel, which unmounts a sheet's content, keeps it, and a
 * thread of its own starts closed. Pure helpers: the tree reads and writes it through these.
 */

/** The first click on a step with children lists its latest three. */
export const FIRST_SHOWN = 3;
/** "Show 10 more" lists that many earlier ones. */
export const SHOW_MORE = 10;
/** A level of more than this many rows is a scroll box, drawn a window at a time. */
export const SCROLL_FROM = 50;

export type ExpansionState = {
  /** The turns open by the person's choice, or because the shell asked to show them. */
  turns: ReadonlySet<string>;
  /** Per node (`<turn id>/<node id>`): how many of its latest children are listed; none is closed. */
  nodes: ReadonlyMap<string, number>;
  /**
   * The person opened or closed a turn: from then on the pane stops following the turn that is
   * live and leaves the others as they are.
   */
  picked: boolean;
  /** The shell's last request to show a turn that the pane has acted on (`focus.key`). */
  seenKey: number;
};

export const NO_EXPANSION: ExpansionState = {
  turns: new Set(),
  nodes: new Map(),
  picked: false,
  seenKey: 0,
};

export const nodeKey = (turnId: string, nodeId: string): string => `${turnId}/${nodeId}`;

/** How many children of a node are listed: none while it is collapsed. */
export const shownOf = (state: ExpansionState, key: string): number => state.nodes.get(key) ?? 0;

export function withShown(state: ExpansionState, key: string, shown: number): ExpansionState {
  const nodes = new Map(state.nodes);
  if (shown <= 0) nodes.delete(key);
  else nodes.set(key, shown);
  return { ...state, nodes };
}

/** The next click on a node's "Show more": the first one lists three, each later one ten more. */
export const showMore = (state: ExpansionState, key: string): ExpansionState =>
  withShown(state, key, shownOf(state, key) === 0 ? FIRST_SHOWN : shownOf(state, key) + SHOW_MORE);

/** Opens a turn the shell asked to show and marks the request as acted on. */
export function withFocused(state: ExpansionState, turnId: string, key: number): ExpansionState {
  return { ...state, turns: new Set(state.turns).add(turnId), seenKey: key };
}

type PathNode = { id: string; kind: string; children: readonly { id: string }[] };

/**
 * Opens the way to a step (a deep link, the failed chip of the chat's line): every step it runs
 * under lists far enough back to include the next one, and the step itself opens (its children, if
 * it has any, else its input and output). What was open stays open, and the turn's own root is not
 * a row, so it has nothing to open.
 */
export function withPathOpen(
  state: ExpansionState,
  turnId: string,
  path: readonly PathNode[],
): ExpansionState {
  let next = state;
  path.forEach((node, i) => {
    if (node.kind === "agent") return;
    const key = nodeKey(turnId, node.id);
    const following = path[i + 1];
    const behind = following ? node.children.findIndex((c) => c.id === following.id) : -1;
    const wanted = following
      ? Math.max(FIRST_SHOWN, behind < 0 ? 0 : node.children.length - behind)
      : node.children.length > 0
        ? FIRST_SHOWN
        : 1;
    next = withShown(next, key, Math.max(shownOf(next, key), wanted));
  });
  return next;
}
