/*
 * The pages of a thread's history the page holds (ADR 0059, docs/api/history.md), as numbers: which events they cover, whether
 * there are older ones, and how many turns to ask for next. Pure: the frames and the messages are the agent's and the
 * runtime's; this only keeps the account, and refuses a page that does not fit it.
 */

/** What a page says of itself (`HistoryPage` of the contract, without its frames). */
export type PageMeta = {
  start: number;
  end: number;
  head: number;
  earlier: boolean;
  projection: number;
};

/** What the web asks for (`ui.history` of `GET /api/config`). */
export type HistoryConfig = {
  /** The turns of the newest page. */
  initialTurns: number;
  /** The turns of the first older page; the next ask for more. */
  pageTurns: number;
  /** The most the server accepts in one page. */
  maxTurns: number;
};

/** A page that does not join the pages held: a hole between them, or an overlap. The account is not changed. */
export class HistoryGap extends Error {
  constructor(
    readonly held: number,
    readonly got: PageMeta,
  ) {
    super(
      `a page of events ${got.start} to ${got.end} does not end where the oldest page held (${held}) begins`,
    );
    this.name = "HistoryGap";
  }
}

/**
 * An older page written by another projection than the pages held (`HistoryPage.projection`): the orchestrator was replaced
 * while the page was open, and frames of two versions are not to be put in one transcript. Reloading the page opens the thread again.
 */
export class ProjectionChanged extends Error {
  constructor(
    readonly held: number,
    readonly got: number,
  ) {
    super(
      `the server was updated while this page was open (frames of version ${held}, now ${got}): reload the page`,
    );
    this.name = "ProjectionChanged";
  }
}

/** The turns of the `n`th older page (0 is the first): `pageTurns`, twice that, four times, up to `maxTurns`. */
export function turnsOfPage(n: number, { pageTurns, maxTurns }: HistoryConfig): number {
  return Math.min(maxTurns, pageTurns * 2 ** Math.min(n, 20));
}

export class HistoryWindow {
  /** The pages held, newest first. */
  private readonly held: PageMeta[];

  constructor(
    newest: PageMeta,
    private readonly config: HistoryConfig,
  ) {
    this.held = [newest];
  }

  private get oldest(): PageMeta {
    return this.held.at(-1) as PageMeta;
  }

  /** The first event the pages held account for. */
  get start(): number {
    return this.oldest.start;
  }

  /** Whether the log has events before the oldest page held. */
  get earlier(): boolean {
    return this.oldest.earlier;
  }

  /** The last event the newest page accounts for, a settled point. */
  get end(): number {
    return (this.held[0] as PageMeta).end;
  }

  /** How many pages are held. */
  get pages(): number {
    return this.held.length;
  }

  /** The turns to ask for next: the older pages held so far set how many. */
  get nextTurns(): number {
    return turnsOfPage(this.held.length - 1, this.config);
  }

  /** The `before` of the next older page: the event the oldest page begins at. */
  get before(): number {
    return this.oldest.start;
  }

  /**
   * Whether an older page joins the ones held: it has to end the event before the oldest one begins, and be written by the
   * projection of the pages held. Throws [`HistoryGap`] or [`ProjectionChanged`]; nothing is changed either way.
   */
  check(page: PageMeta): void {
    if (page.end + 1 !== this.oldest.start || page.start > page.end + 1) {
      throw new HistoryGap(this.oldest.start, page);
    }
    // an empty page that says there is more before it would never end
    if (page.start === this.oldest.start) throw new HistoryGap(this.oldest.start, page);
    if (page.projection !== this.oldest.projection) {
      throw new ProjectionChanged(this.oldest.projection, page.projection);
    }
  }

  /** An older page, which has to join the ones held (`check`). Throws what it throws and keeps what it has when it does not. */
  addOlder(page: PageMeta): void {
    this.check(page);
    this.held.push(page);
  }
}
