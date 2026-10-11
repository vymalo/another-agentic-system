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
import type { KeptFile } from "@/features/chat/lib/files";
import { actionRun, mountSurfaces, resetSeq, stubLayout, surfaceRun } from "./testing";

/*
 * The question types of version 5 of the Choices, each through the real runtime and transcript:
 * the title and description, a help line, a default, a recommended mark, a list that is searched,
 * a deck of pictures, typed answers, the free box and what the person's answer reads like.
 */

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(resetSeq);
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

type Rec = Record<string, unknown>;
type Mounted = ReturnType<typeof mountSurfaces>;

async function feed(m: Mounted, frames: ReturnType<typeof surfaceRun>) {
  await act(async () => {
    m.stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
}

const SHA_A = "a".repeat(64);
const SHA_B = "b".repeat(64);
const image = (sha256: string, filename: string): KeptFile => ({
  sha256,
  href: `/api/threads/t/artifacts/${sha256}`,
  size: 10,
  filename,
  name: filename,
  mimeType: "image/png",
  preview: "image",
});
const FILES = [image(SHA_A, "login-a.png"), image(SHA_B, "login-b.png")];

const opt = (value: string, label = value, extra: Rec = {}) => ({ value, label, ...extra });
const form = (questions: Rec[], extra: Rec = {}) =>
  surface(
    [
      { id: "root", component: "Column", children: ["pick"] },
      { id: "pick", component: "Choices", questions, ...extra },
    ],
    undefined,
    "v0.9.1",
    OWN_CATALOG.catalogId,
  );

async function mountForm(
  questions: Rec[],
  extra: Rec = {},
  host: Parameters<typeof mountSurfaces>[0] = {},
) {
  const m = mountSurfaces(host, {}, undefined, {
    catalogVersion: OWN_CATALOG.version,
    carriedFiles: FILES,
  });
  await feed(m, surfaceRun([form(questions, extra)]));
  return m;
}

const send = () => screen.getByRole("button", { name: "Send answers" }) as HTMLButtonElement;
const click = (el: Element) => fireEvent.click(el);
const sent = (fn: ReturnType<typeof vi.fn>) => fn.mock.calls[0]?.[0] as Rec;

describe("the form: a title, why the agent asks, and a free box", () => {
  it("draws the title as a heading, the description beneath it, and a help line under its question", async () => {
    const m = await mountForm(
      [
        {
          id: "db",
          question: "Which database?",
          help: "The one the service keeps its data in",
          options: [opt("pg", "Postgres"), opt("sqlite", "SQLite")],
        },
      ],
      { title: "Before I start", description: "I need a database to write the migration." },
    );
    expect(screen.getByRole("heading", { name: "Before I start" })).toBeTruthy();
    expect(screen.getByText("I need a database to write the migration.")).toBeTruthy();
    const db = screen.getByRole("radiogroup", { name: "Which database?" });
    // the help describes the group; it is not part of its name
    const described = document.getElementById(db.getAttribute("aria-describedby") ?? "");
    expect(described?.textContent).toContain("The one the service keeps its data in");
    m.agent.stop();
  });

  it("the free box is optional; its words are the answer's note, and nothing is sent without the click", async () => {
    const sendAction = vi.fn();
    const m = await mountForm(
      [{ id: "q", question: "Which?", options: [opt("a", "Alpha"), opt("b", "Beta")] }],
      { freeInput: { label: "Anything else?", placeholder: "A deadline…" } },
      { send: sendAction },
    );
    const box = screen.getByLabelText("Anything else?") as HTMLTextAreaElement;
    expect(box.placeholder).toBe("A deadline…");
    expect(box.maxLength).toBe(2000);
    click(screen.getByRole("radio", { name: "Alpha" }));
    fireEvent.change(box, { target: { value: "  by Friday, please  " } });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(sendAction).not.toHaveBeenCalled();
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "q", values: ["a"] }], note: "by Friday, please" },
    });
    m.agent.stop();
  });

  it("an empty free box sends no note, and does not block", async () => {
    const sendAction = vi.fn();
    const m = await mountForm(
      [{ id: "q", question: "Which?", options: [opt("a", "Alpha"), opt("b", "Beta")] }],
      { freeInput: {} },
      { send: sendAction },
    );
    expect(screen.getByLabelText("Anything else you want to add?")).toBeTruthy();
    click(screen.getByRole("radio", { name: "Beta" }));
    click(send());
    const context = (sent(sendAction) as { context: Rec }).context;
    expect(context).toEqual({ answers: [{ id: "q", values: ["b"] }] });
    m.agent.stop();
  });
});

describe("options: a title, a subtitle, an icon and a recommended mark; a default", () => {
  it("draws the label and the description, the icon, and 'Recommended' as the option's description", async () => {
    const m = await mountForm([
      {
        id: "db",
        question: "Which database?",
        default: "pg",
        options: [
          opt("pg", "Postgres", { description: "Relational", icon: "git", recommended: true }),
          opt("sqlite", "SQLite", { description: "A file" }),
        ],
      },
    ]);
    const pg = screen.getByRole("radio", { name: "Postgres" }) as HTMLInputElement;
    const described = (pg.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .map((id) => document.getElementById(id)?.textContent);
    expect(described).toEqual(["Relational", "Recommended"]);
    expect(screen.getAllByText("Recommended")).toHaveLength(1);
    // the option's icon is drawn, and is not read out
    expect(
      pg.closest('[data-slot="choices-option"]')?.querySelector("svg[aria-hidden]"),
    ).toBeTruthy();
    // the default is chosen before the person does anything, and the form can go as it is
    expect(pg.checked).toBe(true);
    expect(send().disabled).toBe(false);
    m.agent.stop();
  });

  it("a default is sent like a choice, and the person can change it", async () => {
    const sendAction = vi.fn();
    const m = await mountForm(
      [
        {
          id: "db",
          question: "Which database?",
          default: "pg",
          options: [opt("pg", "Postgres"), opt("sqlite", "SQLite")],
        },
        {
          id: "where",
          question: "Where?",
          type: "multiple",
          default: ["k8s", "compose"],
          options: [opt("k8s", "Kubernetes"), opt("compose", "Compose"), opt("vm", "A VM")],
        },
      ],
      {},
      { send: sendAction },
    );
    click(screen.getByRole("checkbox", { name: "Compose" }));
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: {
        answers: [
          { id: "db", values: ["pg"] },
          { id: "where", values: ["k8s"] },
        ],
      },
    });
    m.agent.stop();
  });
});

describe("type multiple: min and max", () => {
  const questions = [
    {
      id: "pick",
      question: "Which two or three?",
      type: "multiple",
      min: 2,
      max: 3,
      options: [opt("a", "Alpha"), opt("b", "Beta"), opt("c", "Gamma"), opt("d", "Delta")],
    },
  ];

  it("says how many to choose, holds the form until min, and stops taking boxes at max", async () => {
    const sendAction = vi.fn();
    const m = await mountForm(questions, {}, { send: sendAction });
    expect(screen.getByText("Choose 2 to 3.")).toBeTruthy();
    const box = (label: string) =>
      screen.getByRole("checkbox", { name: label }) as HTMLInputElement;
    click(box("Alpha"));
    expect(send().disabled).toBe(true);
    click(box("Beta"));
    expect(send().disabled).toBe(false);
    click(box("Gamma"));
    // three chosen: the fourth cannot be taken, the three can be given back
    expect(box("Delta").disabled).toBe(true);
    expect(box("Alpha").disabled).toBe(false);
    click(box("Alpha"));
    expect(box("Delta").disabled).toBe(false);
    click(box("Delta"));
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "pick", values: ["b", "c", "d"] }] },
    });
    m.agent.stop();
  });

  it("the other kinds of hint", async () => {
    const m = await mountForm([
      { ...questions[0], id: "x", min: 1, max: undefined },
      { ...questions[0], id: "y", min: undefined, max: 2 },
      { ...questions[0], id: "z", min: 2, max: 2 },
    ]);
    expect(screen.getByText("Choose at least 1.")).toBeTruthy();
    expect(screen.getByText("Choose up to 2.")).toBeTruthy();
    expect(screen.getByText("Choose 2.")).toBeTruthy();
    m.agent.stop();
  });
});

describe("type select: a list that is searched", () => {
  const FRUIT = [
    opt("apple", "Apple", { description: "Crisp" }),
    opt("apricot", "Apricot"),
    opt("banana", "Banana", { recommended: true }),
    opt("cherry", "Cherry"),
  ];
  const combo = () => screen.getByRole("combobox") as HTMLInputElement;
  const options = () => screen.queryAllByRole("option").map((o) => o.textContent);

  it("is a combobox named by its question; it opens on focus and filters as the person types", async () => {
    const m = await mountForm([
      { id: "f", question: "Which fruit?", type: "select", options: FRUIT },
    ]);
    expect(combo().getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.focus(combo());
    expect(combo().getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByRole("listbox", { name: "Which fruit?" })).toBeTruthy();
    expect(options()).toHaveLength(4);
    fireEvent.change(combo(), { target: { value: "ap" } });
    expect(options().map((o) => o?.slice(0, 5))).toEqual(["Apple", "Apric"]);
    fireEvent.change(combo(), { target: { value: "crisp" } });
    // the subtitle is searched too
    expect(options()).toEqual(["Apple" + "Crisp"]);
    fireEvent.change(combo(), { target: { value: "zzz" } });
    expect(options()).toEqual([]);
    expect(screen.getByText("No match")).toBeTruthy();
    m.agent.stop();
  });

  it("the arrows move, Enter chooses and shows the choice; Enter sends nothing and Escape closes", async () => {
    const sendAction = vi.fn();
    const m = await mountForm(
      [{ id: "f", question: "Which fruit?", type: "select", options: FRUIT }],
      {},
      { send: sendAction },
    );
    fireEvent.focus(combo());
    fireEvent.keyDown(combo(), { key: "ArrowDown" });
    expect(combo().getAttribute("aria-activedescendant")).toBe(
      screen.getAllByRole("option")[1]?.id,
    );
    fireEvent.keyDown(combo(), { key: "ArrowDown" });
    fireEvent.keyDown(combo(), { key: "Enter" });
    // closed, and the box shows what was chosen
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(combo().value).toBe("Banana");
    expect(sendAction).not.toHaveBeenCalled();
    expect(send().disabled).toBe(false);
    fireEvent.focus(combo());
    expect(screen.getAllByRole("option").map((o) => o.getAttribute("aria-selected"))).toEqual([
      "false",
      "false",
      "true",
      "false",
    ]);
    fireEvent.keyDown(combo(), { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "f", values: ["banana"] }] },
    });
    m.agent.stop();
  });

  it("a click chooses; the badge says 'Recommended'; the subtitle is drawn", async () => {
    const m = await mountForm([
      { id: "f", question: "Which fruit?", type: "select", options: FRUIT },
    ]);
    fireEvent.focus(combo());
    expect(within(screen.getByRole("listbox")).getAllByText("Recommended")).toHaveLength(1);
    expect(within(screen.getByRole("listbox")).getByText("Crisp")).toBeTruthy();
    click(screen.getByRole("option", { name: /Cherry/ }));
    expect(combo().value).toBe("Cherry");
    m.agent.stop();
  });

  it("with multiple: true it takes several, shows them as chips that can be removed, and stops at max", async () => {
    const sendAction = vi.fn();
    const m = await mountForm(
      [
        {
          id: "f",
          question: "Which fruit?",
          type: "select",
          multiple: true,
          max: 2,
          options: FRUIT,
        },
      ],
      {},
      { send: sendAction },
    );
    fireEvent.focus(combo());
    expect(screen.getByRole("listbox").getAttribute("aria-multiselectable")).toBe("true");
    click(screen.getByRole("option", { name: /Cherry/ }));
    click(screen.getByRole("option", { name: /Apple/ }));
    // the list stays open, the third is not taken
    expect(screen.getByRole("listbox")).toBeTruthy();
    click(screen.getByRole("option", { name: /Banana/ }));
    expect(screen.getByRole("option", { name: /Banana/ }).getAttribute("aria-disabled")).toBe(
      "true",
    );
    const chips = document.querySelector('[data-slot="choices-chips"]') as HTMLElement;
    expect(within(chips).getAllByRole("listitem")).toHaveLength(2);
    click(within(chips).getByRole("button", { name: "Remove Cherry" }));
    expect(within(chips).getAllByRole("listitem")).toHaveLength(1);
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "f", values: ["apple"] }] },
    });
    m.agent.stop();
  });

  it("holds 50 options", async () => {
    const many = Array.from({ length: 50 }, (_, i) => opt(`v${i}`, `Option ${i}`));
    const m = await mountForm([{ id: "f", question: "Which?", type: "select", options: many }]);
    fireEvent.focus(combo());
    expect(options()).toHaveLength(50);
    fireEvent.change(combo(), { target: { value: "Option 4" } });
    expect(options()).toHaveLength(11); // Option 4 and Option 40 to 49
    m.agent.stop();
  });

  it("is inert when the thread does not wait for the owner", async () => {
    const m = await mountForm(
      [{ id: "f", question: "Which fruit?", type: "select", options: FRUIT }],
      {},
      { canSend: false, state: "working" },
    );
    expect(combo().matches(":disabled")).toBe(true);
    m.agent.stop();
  });
});

describe("type images: a deck of pictures", () => {
  const deck = (extra: Rec = {}) => [
    {
      id: "look",
      question: "Which login looks right?",
      type: "images",
      options: [
        opt("a", "Card", {
          description: "Centered, one field",
          image: { sha256: SHA_A, alt: "A login card in the middle of the page" },
        }),
        opt("b", "Split", {
          image: { sha256: SHA_B, alt: "A login form beside a photo" },
          recommended: true,
        }),
      ],
      ...extra,
    },
  ];

  it("is a radio group of tiles; each shows its picture (from the thread's file, with the agent's alt), label and subtitle", async () => {
    const m = await mountForm(deck());
    const look = screen.getByRole("radiogroup", { name: "Which login looks right?" });
    const tiles = look.querySelectorAll('[data-slot="choices-deck-option"]');
    expect(tiles).toHaveLength(2);
    const first = within(tiles[0] as HTMLElement).getByRole("img", {
      name: "A login card in the middle of the page",
    });
    expect(first.getAttribute("src")).toBe(`/api/threads/t/artifacts/${SHA_A}`);
    expect(
      within(tiles[1] as HTMLElement)
        .getByRole("img", { name: "A login form beside a photo" })
        .getAttribute("src"),
    ).toBe(`/api/threads/t/artifacts/${SHA_B}`);
    expect(within(tiles[0] as HTMLElement).getByText("Centered, one field")).toBeTruthy();
    expect(within(tiles[1] as HTMLElement).getByText("Recommended")).toBeTruthy();
    // the radio is named by the label alone, not by the picture's description
    expect(within(look).getByRole("radio", { name: "Split" })).toBeTruthy();
    m.agent.stop();
  });

  it("choosing a tile chooses its option; one choice replaces the other", async () => {
    const sendAction = vi.fn();
    const m = await mountForm(deck(), {}, { send: sendAction });
    click(screen.getByRole("radio", { name: "Card" }));
    click(screen.getByRole("radio", { name: "Split" }));
    expect((screen.getByRole("radio", { name: "Card" }) as HTMLInputElement).checked).toBe(false);
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "look", values: ["b"] }] },
    });
    m.agent.stop();
  });

  it("with multiple: true the tiles are boxes", async () => {
    const sendAction = vi.fn();
    const m = await mountForm(deck({ multiple: true }), {}, { send: sendAction });
    expect(screen.queryByRole("radiogroup")).toBeNull();
    click(screen.getByRole("checkbox", { name: "Card" }));
    click(screen.getByRole("checkbox", { name: "Split" }));
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "look", values: ["a", "b"] }] },
    });
    m.agent.stop();
  });

  it("a surface that names a file the thread does not hold is refused, not half drawn", async () => {
    const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });
    await feed(m, surfaceRun([form(deck())]));
    expect(await screen.findByText(/not one of this thread's files/)).toBeTruthy();
    expect(screen.queryByRole("radio")).toBeNull();
    m.agent.stop();
  });
});

describe("types text and longText", () => {
  const questions = [
    { id: "name", question: "What is the project called?", type: "text", help: "One word" },
    { id: "story", question: "Tell me more", type: "longText", required: false },
  ];

  it("a text box and a text area, named by their question; the help describes them; the limits are set", async () => {
    const m = await mountForm(questions);
    const name = screen.getByLabelText("What is the project called?") as HTMLInputElement;
    expect(name.tagName).toBe("INPUT");
    expect(name.maxLength).toBe(500);
    expect(document.getElementById(name.getAttribute("aria-describedby") ?? "")?.textContent).toBe(
      "One word",
    );
    const story = screen.getByLabelText(/Tell me more/) as HTMLTextAreaElement;
    expect(story.tagName).toBe("TEXTAREA");
    expect(story.maxLength).toBe(2000);
    expect(screen.getByText("(optional)")).toBeTruthy();
    m.agent.stop();
  });

  it("a required one holds the form until it has words; the words are sent as `text`, trimmed; the optional one may stay empty", async () => {
    const sendAction = vi.fn();
    const m = await mountForm(questions, {}, { send: sendAction });
    const name = screen.getByLabelText("What is the project called?");
    expect(send().disabled).toBe(true);
    fireEvent.change(name, { target: { value: "   " } });
    expect(send().disabled).toBe(true);
    fireEvent.change(name, { target: { value: "  Quokka  " } });
    expect(send().disabled).toBe(false);
    fireEvent.keyDown(name, { key: "Enter" });
    expect(sendAction).not.toHaveBeenCalled();
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: {
        answers: [
          { id: "name", values: [], text: "Quokka" },
          { id: "story", values: [] },
        ],
      },
    });
    m.agent.stop();
  });

  it("a default is the text it starts with", async () => {
    const m = await mountForm([{ id: "name", question: "Name?", type: "text", default: "Quokka" }]);
    expect((screen.getByLabelText("Name?") as HTMLInputElement).value).toBe("Quokka");
    expect(send().disabled).toBe(false);
    m.agent.stop();
  });
});

describe("the person's answer in the transcript", () => {
  const bubble = () => document.querySelector('[data-slot="answer-bubble"]') as HTMLElement;

  it("shows the words typed under their question and the note under its label", async () => {
    const ops = form(
      [
        { id: "name", question: "What is the project called?", type: "text" },
        {
          id: "db",
          question: "Which database?",
          options: [opt("pg", "Postgres"), opt("s", "SQLite")],
        },
      ],
      { freeInput: { label: "Anything else?" } },
    );
    const m = mountSurfaces({ canSend: false, state: "done" }, {}, undefined, {
      catalogVersion: OWN_CATALOG.version,
    });
    const frames = [
      ...surfaceRun([ops], { end: "interrupt" }),
      ...actionRun({
        surfaceId: "s1",
        name: "answer",
        sourceComponentId: "pick",
        context: {
          answers: [
            { id: "name", values: [], text: "Quokka" },
            { id: "db", values: ["pg"] },
          ],
          note: "by Friday\nplease",
        },
      }),
    ];
    await act(async () => {
      m.stream.frames(frames);
    });
    await screen.findByText("Going on.");
    const lines = Array.from(bubble().querySelectorAll('[data-slot="answer-line"]')).map((l) => [
      l.querySelector("dt")?.textContent,
      l.querySelector("dd")?.textContent,
    ]);
    expect(lines).toEqual([
      ["What is the project called?", "Quokka"],
      ["Which database?", "Postgres"],
    ]);
    const note = bubble().querySelector('[data-slot="answer-note"]');
    expect(note?.querySelector("dt")?.textContent).toBe("Anything else?");
    expect(note?.querySelector("dd")?.textContent).toBe("by Friday\nplease");
    m.agent.stop();
  });
});
