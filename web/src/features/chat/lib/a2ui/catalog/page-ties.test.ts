import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  ANNOUNCE_AFTER_MS,
  ANNOUNCE_MAX_WAIT_MS,
  ANNOUNCE_SETTLE_MS,
} from "@/features/chat/components/surface/plan-announcer";
import { MAX_SHOW, MAX_SHOWN_IN_FORM, SHOWABLE } from "../choices";
import { MAX_BYTES, MAX_COMPONENTS } from "../limits";
import { OWN_CATALOG } from "./index";

/**
 * The numbers the contract page states about version 8 are tied to the code: each is read from the page and
 * compared with the constant (or the schema) that the code uses, so that a number cannot be changed in one place
 * only. The page is `docs/api/ui-catalog-v1.md`; a sentence that is reworded breaks the pattern that reads it, and
 * the test says which.
 */

const doc = readFileSync(
  path.resolve(import.meta.dirname, "../../../../../../../docs/api/ui-catalog-v1.md"),
  "utf8",
);
const grab = (re: RegExp, what: string): RegExpExecArray => {
  const m = re.exec(doc);
  expect(m, `the page states ${what}`).not.toBeNull();
  return m as RegExpExecArray;
};
type Rec = Record<string, unknown>;
const props = (name: string): Rec =>
  (OWN_CATALOG.catalog.components[name] as Rec).properties as Rec;

describe("the numbers of the page are the code's", () => {
  it("the ids a question shows: 1 to MAX_SHOW, and the schema says the same", () => {
    const n = Number(
      grab(/\| Ids in the `show` of one question \| 1 to (\d+) \|/, "the ids of a question")[1],
    );
    expect(n).toBe(MAX_SHOW);
    const question = ((props("Choices").questions as Rec).items as Rec).properties as Rec;
    expect((question.show as Rec).maxItems).toBe(n);
    expect((question.show as Rec).minItems).toBe(1);
    expect(grab(/A list of \*\*1 to (\d+)\*\* ids/, "1 to N in the member table")[1]).toBe(
      String(n),
    );
  });

  it("the ids a form shows: at most MAX_SHOWN_IN_FORM", () => {
    const n = Number(
      grab(
        /\| Ids in the `show` of all the questions of one `Choices` \| at most (\d+) \|/,
        "the ids of a form",
      )[1],
    );
    expect(n).toBe(MAX_SHOWN_IN_FORM);
  });

  it("the components a question may show are the closed list of the code, seven of them", () => {
    const sentence = grab(
      /closed list of seven components:\*\*([^.]*)\./,
      "the closed list of seven components",
    )[1] as string;
    const names = [...sentence.matchAll(/`(\w+)`/g)].map((m) => m[1]);
    expect(names).toEqual([...SHOWABLE]);
    expect(SHOWABLE.length).toBe(7);
  });

  it("the note of a checklist item: 200 characters, as the schema says", () => {
    const n = Number(
      grab(
        /\| item `note` \| up to (\d+) characters \(Unicode code points\)/,
        "the length of a note",
      )[1],
    );
    const items = (props("Checklist").items as Rec).items as Rec;
    expect(((items.properties as Rec).note as Rec).maxLength).toBe(n);
  });

  it("the sizes of a surface: 64 KiB of operations and 400 components", () => {
    const kib = Number(
      grab(/\*\*(\d+) KiB\*\* of the operations/, "the size of the operations")[1],
    );
    expect(kib * 1024).toBe(MAX_BYTES);
    expect(Number(grab(/([\d,]+) bytes of UTF-8/, "the bytes")[1]?.replace(",", ""))).toBe(
      MAX_BYTES,
    );
    expect(Number(grab(/\*\*(\d+) components\*\*/, "the components of a surface")[1])).toBe(
      MAX_COMPONENTS,
    );
  });

  it("the announcer: quiet for 0.8 s, and at the latest 5 s", () => {
    const quiet = grab(/quiet for\s+([\d.]+) s/, "the quiet of the announcer")[1];
    expect(Number(quiet) * 1000).toBe(ANNOUNCE_AFTER_MS);
    const settle = grab(
      /within \*\*(\d+) s\*\* of the client first drawing/,
      "the settling time",
    )[1];
    expect(Number(settle) * 1000).toBe(ANNOUNCE_SETTLE_MS);
    const max = grab(/at the latest \*\*(\d+) s\*\* after the burst/, "the maximum wait")[1];
    expect(Number(max) * 1000).toBe(ANNOUNCE_MAX_WAIT_MS);
  });

  it("the components that write `weight` are the ones the page lists, and no others", () => {
    const sentence = grab(
      /it is in ([\s\S]*?) and in no other component/,
      "which components write weight",
    )[1] as string;
    const listed = [...sentence.matchAll(/`(\w+)`/g)].map((m) => m[1]).sort();
    const have = Object.entries(OWN_CATALOG.catalog.components)
      .filter(([, schema]) => "weight" in ((schema as Rec).properties as Rec))
      .map(([name]) => name)
      .sort();
    expect(listed).toEqual(have);
  });
});
