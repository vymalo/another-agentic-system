// @vitest-environment jsdom
import {
  act,
  cleanup,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { OWN_CATALOG } from "@/features/chat/lib/a2ui/catalog";
import { surface } from "@/features/chat/lib/a2ui/testing";
import { mountSurfaces, resetSeq, stubLayout, surfaceRun } from "./testing";

/*
 * A question that shows what it asks about (catalog version 8, docs/api/ui-catalog-v1.md, "A question
 * shows"), through the real runtime, transcript and renderers: the shown components are drawn inside the
 * question's group, after its words and before its control, they read when the form no longer takes input,
 * and the answer is what it was. A surface that names what may not be shown is refused whole, in words.
 * Images and diagrams are drawn in the browser (e2e/choices-v8.spec.ts).
 */

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(resetSeq);
afterEach(cleanup);

type Rec = Record<string, unknown>;
type Mounted = ReturnType<typeof mountSurfaces>;

async function feed(m: Mounted, frames: ReturnType<typeof surfaceRun>) {
  await act(async () => {
    m.stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
}

const ops = (components: Rec[]) => surface(components, undefined, "v0.9.1", OWN_CATALOG.catalogId);

async function mountShowing(components: Rec[], host: Parameters<typeof mountSurfaces>[0] = {}) {
  const m = mountSurfaces(host, {}, undefined, { catalogVersion: OWN_CATALOG.version });
  await feed(m, surfaceRun([ops(components)]));
  return m;
}

const opt = (value: string, label = value) => ({ value, label });
const pick = (questions: Rec[]): Rec[] => [
  { id: "root", component: "Column", children: ["pick"] },
  { id: "pick", component: "Choices", questions },
];
const text = (id: string, value: string): Rec => ({ id, component: "Text", text: value });
const code: Rec = { id: "snippet", component: "Code", code: "let x = 1;", title: "Current code" };
const callout: Rec = {
  id: "warn",
  component: "Callout",
  title: "Heads up",
  text: "It cannot be undone.",
  tone: "warning",
};

describe("what a question shows", () => {
  const q = {
    id: "db",
    question: "Which database?",
    help: "Pick one.",
    options: [opt("pg", "Postgres"), opt("my", "MySQL")],
    show: ["why", "snippet"],
  };
  const components = [...pick([q]), text("why", "Postgres is what the other services use."), code];

  it("is drawn inside the question's group, in the order listed, between its words and its choices", async () => {
    const m = await mountShowing(components);
    const group = screen.getByRole("radiogroup", { name: "Which database?" });
    const why = within(group).getByText("Postgres is what the other services use.");
    const copy = within(group).getByRole("button", { name: "Copy code" });
    const radio = within(group).getByRole("radio", { name: "Postgres" });
    const legend = within(group).getByText("Which database?");
    const help = within(group).getByText("Pick one.");
    const follows = (a: Node, b: Node) =>
      Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    // the question, its help, what it shows (in the order of show), then its control
    expect(follows(legend, help)).toBe(true);
    expect(follows(help, why)).toBe(true);
    expect(follows(why, copy)).toBe(true);
    expect(follows(copy, radio)).toBe(true);
    // and the same things are not drawn a second time anywhere else
    expect(screen.getAllByText("Postgres is what the other services use.")).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "Copy code" })).toHaveLength(1);
    m.agent.stop();
  });

  it("is read when the form no longer takes input: the choices are off and the Copy button is not", async () => {
    const m = await mountShowing(components, { canSend: false });
    const group = screen.getByRole("radiogroup", { name: "Which database?" });
    // a control in a disabled fieldset is disabled by inheritance, which `:disabled` sees and `.disabled` does not
    expect(within(group).getByRole("radio", { name: "Postgres" }).matches(":disabled")).toBe(true);
    expect(within(group).getByRole("button", { name: "Copy code" }).matches(":disabled")).toBe(
      false,
    );
    m.agent.stop();
  });

  it("does not change the answer: one action, the same context a question without show sends", async () => {
    const send = vi.fn();
    const m = await mountShowing(components, { send });
    fireEvent.click(screen.getByRole("radio", { name: "MySQL" }));
    fireEvent.click(screen.getByRole("button", { name: "Send answers" }));
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0]?.[0]).toMatchObject({
      name: "answer",
      sourceComponentId: "pick",
      context: { answers: [{ id: "db", values: ["my"] }] },
    });
    m.agent.stop();
  });

  it("a typed question that shows something is a group named by its words", async () => {
    const m = await mountShowing([
      ...pick([{ id: "why", question: "Why this one?", type: "text", show: ["note"] }]),
      { ...callout, id: "note" },
    ]);
    const group = screen.getByRole("group", { name: "Why this one?" });
    expect(within(group).getByText("It cannot be undone.")).toBeTruthy();
    expect(within(group).getByRole("textbox", { name: "Why this one?" })).toBeTruthy();
    m.agent.stop();
  });

  it("a yes or no and a rank show too", async () => {
    const m = await mountShowing([
      ...pick([
        { id: "go", question: "Ship it?", type: "confirm", show: ["a"] },
        {
          id: "order",
          question: "Put these in order",
          type: "rank",
          options: [opt("x", "Speed"), opt("y", "Cost")],
          show: ["b"],
        },
      ]),
      text("a", "Tests pass."),
      text("b", "Both matter."),
    ]);
    expect(
      within(screen.getByRole("radiogroup", { name: "Ship it?" })).getByText("Tests pass."),
    ).toBeTruthy();
    expect(
      within(screen.getByRole("group", { name: "Put these in order" })).getByText("Both matter."),
    ).toBeTruthy();
    m.agent.stop();
  });

  it("a question that shows nothing is drawn as it was: no wrapper, the group itself is the disabled fieldset", async () => {
    const m = await mountShowing(
      pick([{ id: "db", question: "Which?", options: [opt("a"), opt("b")] }]),
      {
        canSend: false,
      },
    );
    const group = screen.getByRole("radiogroup", { name: "Which?" });
    expect(group.tagName).toBe("FIELDSET");
    expect((group as HTMLFieldSetElement).disabled).toBe(true);
    expect(group.querySelector("[data-slot=choices-shown]")).toBeNull();
    m.agent.stop();
  });
});

describe("what a question may not show", () => {
  const refusal = () => document.querySelector("[data-rule]");

  it("another Choices: the whole surface is refused, rule schema, in words", async () => {
    const m = await mountShowing([
      ...pick([{ id: "a", question: "A?", options: [opt("x"), opt("y")], show: ["other"] }]),
      { id: "other", component: "Choices", questions: [{ id: "b", question: "B?", type: "text" }] },
    ]);
    expect(refusal()?.getAttribute("data-rule")).toBe("schema");
    expect(screen.getByText(/is shown by the question "a" of "pick"/)).toBeTruthy();
    expect(screen.queryByText("A?")).toBeNull();
    m.agent.stop();
  });

  it("a component that is also placed in the surface: rule expansion", async () => {
    const m = await mountShowing([
      { id: "root", component: "Column", children: ["pick", "why"] },
      {
        id: "pick",
        component: "Choices",
        questions: [{ id: "a", question: "A?", options: [opt("x"), opt("y")], show: ["why"] }],
      },
      text("why", "Because."),
    ]);
    expect(refusal()?.getAttribute("data-rule")).toBe("expansion");
    m.agent.stop();
  });

  it("an id the surface does not hold yet: not complete, nothing drawn, and drawn when it arrives", async () => {
    const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });
    const first = ops(
      pick([{ id: "a", question: "A?", options: [opt("x"), opt("y")], show: ["why"] }]),
    );
    const second = [
      {
        version: "v0.9.1",
        updateComponents: { surfaceId: "s1", components: [text("why", "Because.")] },
      },
    ];
    await feed(m, surfaceRun([first]));
    expect(screen.getByText("The interface is not complete yet.")).toBeTruthy();
    expect(screen.queryByText("A?")).toBeNull();
    await feed(m, surfaceRun([[...first, ...second]], { runId: "run-1b", user: false }));
    await waitFor(() => expect(screen.getByText("Because.")).toBeTruthy());
    expect(
      within(screen.getByRole("radiogroup", { name: "A?" })).getByText("Because."),
    ).toBeTruthy();
    m.agent.stop();
  });
});

describe("the order of what a question shows, for every kind of question", () => {
  const WORDS = "Which one, exactly?";
  const kinds: [string, Rec][] = [
    ["single", { options: [opt("a"), opt("b")] }],
    ["multiple", { options: [opt("a"), opt("b")] }],
    ["select", { options: [opt("a"), opt("b")] }],
    ["text", {}],
    ["longText", {}],
    ["confirm", {}],
    ["number", {}],
    ["scale", { from: 0, to: 5 }],
    ["date", {}],
    ["rank", { options: [opt("a"), opt("b")] }],
  ];

  for (const [type, extra] of kinds) {
    it(`${type}: the words of the question, then what it shows, then its control`, async () => {
      const m = await mountShowing([
        ...pick([{ id: "q", question: WORDS, help: "A help line.", type, show: ["s"], ...extra }]),
        text("s", "SHOWN-BLOCK"),
      ]);
      const shown = document.querySelector('[data-slot="choices-shown"]') as HTMLElement;
      expect(shown?.textContent).toContain("SHOWN-BLOCK");
      const group = shown.closest("fieldset, [data-slot=choices-question]") as HTMLElement;
      const follows = (a: Node, b: Node) =>
        Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
      const words = within(group).getByText(WORDS, { exact: false });
      expect(follows(words, shown), "the words come before what is shown").toBe(true);
      const controls = [
        ...group.querySelectorAll("input, textarea, select, button, [role=combobox]"),
      ].filter((el) => !shown.contains(el));
      expect(controls.length, "the question has a control").toBeGreaterThan(0);
      for (const control of controls) {
        expect(
          follows(shown, control),
          `what is shown comes before <${control.tagName.toLowerCase()}>`,
        ).toBe(true);
      }
      m.agent.stop();
    });
  }
});
