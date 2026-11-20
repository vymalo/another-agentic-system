import { describe, expect, it } from "vitest";
import { MAX_SHOW, readChoices, showIn } from "./choices";

/*
 * `showIn` is the check of what a form shows that JSON Schema cannot say, and the schema of `Choices` says the same
 * bounds again (`maxItems`, in catalog.json): two places, kept on purpose (the schema is the contract's, `showIn` is what
 * the validator can say about a read form and names the place in words). Each is tried alone here; the schema's numbers
 * are tied to the page in catalog/page-ties.test.ts.
 */

const spec = (shows: string[][]) => {
  const read = readChoices({
    id: "pick",
    component: "Choices",
    questions: shows.map((show, i) => ({
      id: `q${i}`,
      question: `Question ${i}?`,
      type: "text",
      ...(show.length > 0 ? { show } : {}),
    })),
  });
  if (!read) throw new Error("not a Choices");
  return read;
};
const ids = (n: number, from = 0) => Array.from({ length: n }, (_, i) => `id${from + i}`);

describe("showIn, on its own", () => {
  it("takes up to MAX_SHOW ids a question and up to MAX_SHOWN_IN_FORM a form", () => {
    expect(showIn(spec([ids(MAX_SHOW)]))).toBeUndefined();
    expect(showIn(spec([ids(MAX_SHOW), ids(MAX_SHOW, MAX_SHOW)]))).toBeUndefined();
    expect(showIn(spec([[]]))).toBeUndefined();
  });

  it("refuses more than MAX_SHOW in a question", () => {
    expect(showIn(spec([ids(MAX_SHOW + 1)]))).toMatch(/the most is 4/);
  });

  it("refuses more than MAX_SHOWN_IN_FORM in a form", () => {
    expect(showIn(spec([ids(4), ids(4, 4), ids(1, 8)]))).toMatch(/at most 8/);
  });

  it("refuses an id twice in a question and an id in two questions", () => {
    expect(showIn(spec([["a", "a"]]))).toMatch(/twice/);
    expect(showIn(spec([["a"], ["a"]]))).toMatch(/shown by two questions/);
  });
});
