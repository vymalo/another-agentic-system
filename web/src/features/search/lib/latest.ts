/**
 * Which request is the latest. Every request takes a number; an answer is drawn only while its number is still the latest, so a slow
 * answer for an earlier text never replaces the answer for the text that is in the box now (an aborted request is dropped too,
 * but a response that was already on its way is not always abortable).
 */
export class Latest {
  private n = 0;

  /** A request starts: the number it holds, and the latest from now on. */
  next(): number {
    this.n += 1;
    return this.n;
  }

  /** Whether `n` is still the latest request. */
  is(n: number): boolean {
    return n === this.n;
  }

  /** Nothing in flight counts any more (the box was cleared). */
  cancel(): void {
    this.n += 1;
  }
}
