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
import { actionRun, mountSurfaces, resetSeq, stubLayout, surfaceRun } from "./testing";

/*
 * The five kinds of question of catalog version 7 (confirm, number, scale, date, rank), each through
 * the real runtime and transcript: what the person sees, what holds the button, what is sent, and
 * how the answer reads in the transcript. The arrow keys of a radio group are the browser's own
 * and are tried in e2e/choices-v7.spec.ts, with the rest of the keyboard.
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
  const m = mountSurfaces(host, {}, undefined, { catalogVersion: OWN_CATALOG.version });
  await feed(m, surfaceRun([form(questions, extra)]));
  return m;
}

const send = () => screen.getByRole("button", { name: "Send answers" }) as HTMLButtonElement;
const click = (el: Element) => fireEvent.click(el);
const sent = (fn: ReturnType<typeof vi.fn>) => fn.mock.calls[0]?.[0] as Rec;
const opt = (value: string, label = value, extra: Rec = {}) => ({ value, label, ...extra });

describe("confirm: a yes or no in the agent's own words", () => {
  const q = {
    id: "go",
    question: "Delete the old branch?",
    help: "It cannot be restored.",
    type: "confirm",
    yesLabel: "Delete it",
    noLabel: "Keep it",
    default: "no",
  };

  it("is a radio group of the two phrases, named by the question and described by its help, with the safe one chosen", async () => {
    const m = await mountForm([q]);
    const group = screen.getByRole("radiogroup", { name: "Delete the old branch?" });
    const described = document.getElementById(group.getAttribute("aria-describedby") ?? "");
    expect(described?.textContent).toBe("It cannot be restored.");
    const radios = within(group).getAllByRole("radio") as HTMLInputElement[];
    expect(radios.map((r) => r.labels?.[0]?.textContent)).toEqual(["Delete it", "Keep it"]);
    expect(radios.map((r) => r.checked)).toEqual([false, true]);
    // the safe default is an answer: the form can go
    expect(send().disabled).toBe(false);
    m.agent.stop();
  });

  it("sends `yes` or `no`, whatever the words say", async () => {
    const sendAction = vi.fn();
    const m = await mountForm([q], {}, { send: sendAction });
    click(screen.getByRole("radio", { name: "Delete it" }));
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "go", values: ["yes"] }] },
    });
    m.agent.stop();
  });

  it("has no default without one: neither is chosen and the form waits; the words default to Yes and No", async () => {
    const m = await mountForm([{ id: "ok", question: "Go on?", type: "confirm" }]);
    const radios = screen.getAllByRole("radio") as HTMLInputElement[];
    expect(radios.map((r) => r.labels?.[0]?.textContent)).toEqual(["Yes", "No"]);
    expect(radios.some((r) => r.checked)).toBe(false);
    expect(send().disabled).toBe(true);
    expect(screen.getByText("1 question is not answered yet.")).toBeTruthy();
    m.agent.stop();
  });
});

describe("number: a decimal that cannot be out of range", () => {
  const q = {
    id: "budget",
    question: "What is the monthly budget?",
    type: "number",
    unit: "EUR",
    minValue: "0",
    maxValue: "1000",
    step: "0.5",
  };
  const box = () => screen.getByRole("textbox", { name: /What is the monthly budget/ });

  it("is a text box for decimals with its unit in the name and its bounds said in words", async () => {
    const m = await mountForm([q]);
    const input = box() as HTMLInputElement;
    expect(input.inputMode).toBe("decimal");
    expect(input.type).toBe("text");
    expect(input.labels?.[0]?.textContent).toContain("in EUR");
    expect(
      document.getElementById(input.getAttribute("aria-describedby")?.split(" ")[0] ?? "")
        ?.textContent,
    ).toBe("From 0 to 1000 EUR. In steps of 0.5.");
    expect(send().disabled).toBe(true);
    m.agent.stop();
  });

  it("an out-of-range number holds the button and says why, in words linked to the box", async () => {
    const m = await mountForm([q]);
    fireEvent.change(box(), { target: { value: "1500" } });
    expect(send().disabled).toBe(true);
    const message = document.querySelector('[data-slot="choices-problem"]') as HTMLElement;
    expect(message.textContent).toContain("The most is 1000 EUR.");
    expect(box().getAttribute("aria-invalid")).toBe("true");
    expect(box().getAttribute("aria-describedby")).toContain(message.id);
    fireEvent.change(box(), { target: { value: "-1" } });
    expect(message.textContent).toContain("The least is 0 EUR.");
    fireEvent.change(box(), { target: { value: "250.25" } });
    expect(message.textContent).toContain("Use steps of 0.5 EUR from 0.");
    fireEvent.change(box(), { target: { value: "1e3" } });
    expect(message.textContent).toContain("digits and a point");
    fireEvent.change(box(), { target: { value: "1,5" } });
    expect(send().disabled).toBe(true);
    m.agent.stop();
  });

  it("a valid number clears the message, and goes as the string the person typed, trimmed", async () => {
    const sendAction = vi.fn();
    const m = await mountForm([q], {}, { send: sendAction });
    fireEvent.change(box(), { target: { value: "5000" } });
    fireEvent.change(box(), { target: { value: " 250.50 " } });
    expect(document.querySelector('[data-slot="choices-problem"]')?.textContent).toBe("");
    expect(box().getAttribute("aria-invalid")).toBeNull();
    expect(send().disabled).toBe(false);
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "budget", values: ["250.50"] }] },
    });
    m.agent.stop();
  });

  it("an optional number may stay empty, and then sends `values: []`", async () => {
    const sendAction = vi.fn();
    const m = await mountForm([{ ...q, required: false }], {}, { send: sendAction });
    expect(screen.getByText("(optional)")).toBeTruthy();
    expect(send().disabled).toBe(false);
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "budget", values: [] }] },
    });
    m.agent.stop();
  });

  it("starts from its default", async () => {
    const m = await mountForm([{ ...q, default: "250" }]);
    expect((box() as HTMLInputElement).value).toBe("250");
    expect(send().disabled).toBe(false);
    m.agent.stop();
  });
});

describe("scale: a radio group of the points", () => {
  const q = {
    id: "nps",
    question: "How likely are you to recommend it?",
    type: "scale",
    from: 0,
    to: 10,
    fromLabel: "Not at all",
    toLabel: "Certainly",
  };

  it("has a radio button for each point, the two ends carrying their words as a description", async () => {
    const m = await mountForm([q]);
    const group = screen.getByRole("radiogroup", { name: "How likely are you to recommend it?" });
    const radios = within(group).getAllByRole("radio") as HTMLInputElement[];
    expect(radios.map((r) => r.value)).toEqual(Array.from({ length: 11 }, (_, i) => String(i)));
    expect(radios.map((r) => r.labels?.[0]?.textContent)).toEqual(radios.map((r) => r.value));
    const describe = (r: HTMLInputElement) =>
      document.getElementById(r.getAttribute("aria-describedby") ?? "")?.textContent;
    expect(describe(radios[0] as HTMLInputElement)).toBe("Not at all");
    expect(describe(radios[10] as HTMLInputElement)).toBe("Certainly");
    expect(radios[5]?.getAttribute("aria-describedby")).toBeNull();
    m.agent.stop();
  });

  it("sends the point as a string; one starts from 1 when the question does not say", async () => {
    const sendAction = vi.fn();
    const m = await mountForm([{ ...q, from: undefined, to: 5 }], {}, { send: sendAction });
    const radios = screen.getAllByRole("radio") as HTMLInputElement[];
    expect(radios.map((r) => r.value)).toEqual(["1", "2", "3", "4", "5"]);
    click(radios[3] as HTMLInputElement);
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "nps", values: ["4"] }] },
    });
    m.agent.stop();
  });
});

describe("date: a calendar day, as the browser's own control gives it", () => {
  const q = {
    id: "when",
    question: "When should it ship?",
    type: "date",
    minDate: "2026-12-01",
    maxDate: "2026-12-31",
  };
  const box = () => screen.getByLabelText(/When should it ship/) as HTMLInputElement;

  it("is a date control with the bounds as its min and max", async () => {
    const m = await mountForm([q]);
    expect(box().type).toBe("date");
    expect(box().min).toBe("2026-12-01");
    expect(box().max).toBe("2026-12-31");
    expect(send().disabled).toBe(true);
    m.agent.stop();
  });

  it("holds a four-digit year when the question names no latest day", async () => {
    const m = await mountForm([{ id: "any", question: "Any day?", type: "date" }]);
    expect((screen.getByLabelText("Any day?") as HTMLInputElement).max).toBe("9999-12-31");
    m.agent.stop();
  });

  it("a day outside the bounds says which one it broke; a day inside goes as YYYY-MM-DD", async () => {
    const sendAction = vi.fn();
    const m = await mountForm([q], {}, { send: sendAction });
    fireEvent.change(box(), { target: { value: "2027-01-02" } });
    expect(send().disabled).toBe(true);
    expect(document.querySelector('[data-slot="choices-problem"]')?.textContent).toContain(
      "The latest day is 2026-12-31.",
    );
    fireEvent.change(box(), { target: { value: "2026-11-30" } });
    expect(document.querySelector('[data-slot="choices-problem"]')?.textContent).toContain(
      "The earliest day is 2026-12-01.",
    );
    fireEvent.change(box(), { target: { value: "2026-12-15" } });
    expect(send().disabled).toBe(false);
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "when", values: ["2026-12-15"] }] },
    });
    m.agent.stop();
  });
});

describe("rank: put the options in order", () => {
  const q = {
    id: "order",
    question: "Put these in order of importance",
    help: "The most important first.",
    type: "rank",
    options: [
      opt("speed", "Speed"),
      opt("cost", "Cost", { description: "What it costs a month" }),
      opt("risk", "Risk", { recommended: true }),
    ],
  };
  const labels = () =>
    Array.from(document.querySelectorAll('[data-slot="choices-rank-item"]')).map(
      (li) => li.querySelector("span.flex-1 span span")?.textContent,
    );
  const move = (label: string, way: "up" | "down") =>
    click(screen.getByRole("button", { name: `Move ${label} ${way}` }));

  it("is an ordered list in the agent's order with Move up and Move down for each item, named with the item", async () => {
    const m = await mountForm([q]);
    expect(labels()).toEqual(["Speed", "Cost", "Risk"]);
    const list = screen.getByRole("list", { name: /Put these in order of importance/ });
    expect(within(list).getAllByRole("listitem")).toHaveLength(3);
    for (const label of ["Speed", "Cost", "Risk"]) {
      expect(screen.getByRole("button", { name: `Move ${label} up` })).toBeTruthy();
      expect(screen.getByRole("button", { name: `Move ${label} down` })).toBeTruthy();
    }
    expect(screen.getByText("What it costs a month")).toBeTruthy();
    expect(screen.getByText("Recommended")).toBeTruthy();
    m.agent.stop();
  });

  it("is not answered until an item has moved or the order is kept: the agent's order is not their ranking", async () => {
    const sendAction = vi.fn();
    const m = await mountForm([q], {}, { send: sendAction });
    expect(send().disabled).toBe(true);
    expect(screen.getByText("Move an item, or keep the order, to answer.")).toBeTruthy();
    click(screen.getByRole("button", { name: "Keep this order" }));
    expect(send().disabled).toBe(false);
    expect(screen.queryByRole("button", { name: "Keep this order" })).toBeNull();
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "order", values: ["speed", "cost", "risk"] }] },
    });
    m.agent.stop();
  });

  it("sends every id in the order the buttons left them in, and says the new place in words", async () => {
    const sendAction = vi.fn();
    const m = await mountForm([q], {}, { send: sendAction });
    move("Risk", "up");
    expect(labels()).toEqual(["Speed", "Risk", "Cost"]);
    expect(screen.getByRole("status").textContent).toBe("Risk, 2 of 3");
    move("Risk", "up");
    expect(labels()).toEqual(["Risk", "Speed", "Cost"]);
    expect(screen.getByRole("status").textContent).toBe("Risk, 1 of 3");
    move("Cost", "up");
    expect(labels()).toEqual(["Risk", "Cost", "Speed"]);
    expect(send().disabled).toBe(false);
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: { answers: [{ id: "order", values: ["risk", "cost", "speed"] }] },
    });
    m.agent.stop();
  });

  it("a button at an end of the list does nothing, says so, and keeps the keyboard's place", async () => {
    const m = await mountForm([q]);
    const top = screen.getByRole("button", { name: "Move Speed up" });
    expect(top.getAttribute("aria-disabled")).toBe("true");
    expect((top as HTMLButtonElement).disabled).toBe(false);
    top.focus();
    click(top);
    expect(labels()).toEqual(["Speed", "Cost", "Risk"]);
    expect(document.activeElement).toBe(top);
    const bottom = screen.getByRole("button", { name: "Move Risk down" });
    expect(bottom.getAttribute("aria-disabled")).toBe("true");
    m.agent.stop();
  });

  it("the keyboard stays on the button it used after the item has moved", async () => {
    const m = await mountForm([q]);
    const button = screen.getByRole("button", { name: "Move Cost down" });
    button.focus();
    click(button);
    expect(labels()).toEqual(["Speed", "Risk", "Cost"]);
    expect((document.activeElement as HTMLElement).getAttribute("aria-label")).toBe(
      "Move Cost down",
    );
    m.agent.stop();
  });

  it("starts in its default order, which is an answer; an optional rank may be left alone", async () => {
    const sendAction = vi.fn();
    const m = await mountForm(
      [
        { ...q, default: ["risk", "speed", "cost"] },
        { ...q, id: "later", question: "Order these later", required: false },
      ],
      {},
      { send: sendAction },
    );
    expect(send().disabled).toBe(false);
    click(send());
    expect(sent(sendAction)).toMatchObject({
      context: {
        answers: [
          { id: "order", values: ["risk", "speed", "cost"] },
          { id: "later", values: [] },
        ],
      },
    });
    m.agent.stop();
  });
});

describe("the form with all five, once answered", () => {
  const questions: Rec[] = [
    { id: "ship", question: "Delete the old branch?", type: "confirm", yesLabel: "Delete it" },
    { id: "budget", question: "What is the monthly budget?", type: "number", unit: "EUR" },
    {
      id: "nps",
      question: "How likely?",
      type: "scale",
      to: 10,
      fromLabel: "Not at all",
      toLabel: "Certainly",
    },
    { id: "when", question: "When should it ship?", type: "date" },
    {
      id: "order",
      question: "Order these",
      type: "rank",
      options: [opt("speed", "Speed"), opt("cost", "Cost"), opt("risk", "Risk")],
    },
    { id: "skip", question: "Anything?", type: "number", required: false },
  ];

  it("is read back as the person saw it, the rank as a numbered list, a skipped question as no answer", async () => {
    const m = mountSurfaces({ canSend: false, state: "done" }, {}, undefined, {
      catalogVersion: OWN_CATALOG.version,
    });
    const frames = [
      ...surfaceRun([form(questions)], { end: "interrupt" }),
      ...actionRun({
        surfaceId: "s1",
        name: "answer",
        sourceComponentId: "pick",
        context: {
          answers: [
            { id: "ship", values: ["yes"] },
            { id: "budget", values: ["250.5"] },
            { id: "nps", values: ["10"] },
            { id: "when", values: ["2026-12-15"] },
            { id: "order", values: ["risk", "speed", "cost"] },
            { id: "skip", values: [] },
          ],
        },
      }),
    ];
    await act(async () => {
      m.stream.frames(frames);
    });
    await screen.findByText("Going on.");
    const bubble = document.querySelector('[data-slot="answer-bubble"]') as HTMLElement;
    const lines = Array.from(bubble.querySelectorAll('[data-slot="answer-line"]')).map((l) => [
      l.querySelector("dt")?.textContent,
      l.querySelector("dd")?.textContent,
    ]);
    expect(lines).toEqual([
      ["Delete the old branch?", "Delete it"],
      ["What is the monthly budget?", "250.5 EUR"],
      ["How likely?", "10 (Certainly)"],
      ["When should it ship?", "15 December 2026"],
      ["Order these", "RiskSpeedCost"],
      ["Anything?", "No answer"],
    ]);
    const rank = bubble.querySelector('[data-slot="answer-rank"]') as HTMLElement;
    expect(rank.tagName).toBe("OL");
    expect(Array.from(rank.querySelectorAll("li")).map((li) => li.textContent)).toEqual([
      "Risk",
      "Speed",
      "Cost",
    ]);
    m.agent.stop();
  });

  it("stays on the screen as it was answered, and takes no more input", async () => {
    const m = await mountForm(questions, {}, { canSend: false, state: "working" });
    for (const radio of screen.getAllByRole("radio")) expect(radio.matches(":disabled")).toBe(true);
    expect(
      (screen.getByRole("textbox", { name: /monthly budget/ }) as HTMLInputElement).disabled,
    ).toBe(true);
    expect((screen.getByLabelText(/When should it ship/) as HTMLInputElement).disabled).toBe(true);
    for (const button of screen.getAllByRole("button", { name: /^Move / })) {
      expect((button as HTMLButtonElement).disabled).toBe(true);
    }
    m.agent.stop();
  });
});
