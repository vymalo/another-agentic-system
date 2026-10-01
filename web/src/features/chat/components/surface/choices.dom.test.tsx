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

const THREE: Rec[] = [
  {
    id: "db",
    question: "Which database?",
    allowOther: true,
    options: [
      { value: "pg", label: "Postgres", description: "Relational, the default" },
      { value: "sqlite", label: "SQLite" },
    ],
  },
  {
    id: "auth",
    question: "Which login?",
    options: [
      { value: "keycloak", label: "Keycloak" },
      { value: "none", label: "No login" },
    ],
  },
  {
    id: "deploy",
    question: "Where does it run?",
    multiple: true,
    required: false,
    allowOther: true,
    options: [
      { value: "k8s", label: "Kubernetes" },
      { value: "compose", label: "Docker Compose" },
    ],
  },
];

/** The operations of a surface that is one Choices (id `pick`) under a title. */
const choicesSurface = (extra: Rec = {}, questions: Rec[] = THREE) =>
  surface(
    [
      { id: "root", component: "Column", children: ["intro", "pick"] },
      { id: "intro", component: "Text", text: "A few quick choices" },
      { id: "pick", component: "Choices", questions, ...extra },
    ],
    undefined,
    "v0.9.1",
    OWN_CATALOG.catalogId,
  );

const send = () => screen.getByRole("button", { name: "Send answers" }) as HTMLButtonElement;
const group = (name: string) => screen.getByRole("radiogroup", { name }) as HTMLElement;
const radio = (question: string, label: string) =>
  within(group(question)).getByRole("radio", { name: label }) as HTMLInputElement;
const checkbox = (label: string) =>
  screen.getByRole("checkbox", { name: label }) as HTMLInputElement;
const click = (el: Element) => fireEvent.click(el);

async function mountChoices(host: Parameters<typeof mountSurfaces>[0] = {}, extra: Rec = {}) {
  const m = mountSurfaces(host, {}, undefined, { catalogVersion: OWN_CATALOG.version });
  await feed(m, surfaceRun([choicesSurface(extra)]));
  return m;
}

describe("the Choices component", () => {
  it("is a group of radio buttons per question, each named by its question, with the option's own words", async () => {
    const m = await mountChoices();
    expect(screen.getByText("A few quick choices")).toBeTruthy();
    // single choice: a radiogroup of radios
    expect(
      within(group("Which database?"))
        .getAllByRole("radio")
        .map((r) => (r as HTMLInputElement).value),
    ).toEqual(["pg", "sqlite", "other"]);
    expect(radio("Which login?", "Keycloak").checked).toBe(false);
    // several: a plain group of checkboxes, named by its question
    const several = screen.getByRole("group", { name: /Where does it run\?/ });
    expect(
      within(several)
        .getAllByRole("checkbox")
        .map((c) => (c as HTMLInputElement).value),
    ).toEqual(["k8s", "compose", "other"]);
    // a description is the control's description, not part of its name
    const pg = radio("Which database?", "Postgres");
    expect(pg.getAttribute("aria-describedby")).toBeTruthy();
    expect(document.getElementById(pg.getAttribute("aria-describedby") ?? "")?.textContent).toBe(
      "Relational, the default",
    );
    // an optional question says so
    expect(within(several).getByText("(optional)")).toBeTruthy();
    // the radios of one question share a name; two questions never do
    expect(radio("Which login?", "Keycloak").name).not.toBe(
      radio("Which database?", "Postgres").name,
    );
    expect(radio("Which login?", "Keycloak").name).toBe(radio("Which login?", "No login").name);
    m.agent.stop();
  });

  it("sends nothing by being drawn, by choosing, by typing, or by waiting", async () => {
    const sendAction = vi.fn();
    const m = await mountChoices({ send: sendAction });
    vi.useFakeTimers({ toFake: ["setTimeout", "setInterval", "Date"] });
    const own = screen.getByLabelText("Your own answer to: Which database?");
    click(radio("Which database?", "Postgres"));
    click(radio("Which login?", "Keycloak"));
    fireEvent.change(own, { target: { value: "Cockroach" } });
    fireEvent.keyDown(own, { key: "Enter" });
    fireEvent.keyUp(own, { key: "Enter" });
    await act(async () => {
      vi.advanceTimersByTime(60_000);
    });
    expect(sendAction).not.toHaveBeenCalled();
    m.agent.stop();
  });

  it("Send is off until every required question is answered, and says how many are left", async () => {
    const m = await mountChoices();
    expect(send().disabled).toBe(true);
    expect(screen.getByText("2 questions are not answered yet.")).toBeTruthy();
    expect(send().getAttribute("aria-describedby")).toBe(
      screen.getByText("2 questions are not answered yet.").id,
    );
    click(radio("Which database?", "Postgres"));
    expect(screen.getByText("1 question is not answered yet.")).toBeTruthy();
    expect(send().disabled).toBe(true);
    // the optional question never blocks
    click(radio("Which login?", "No login"));
    expect(send().disabled).toBe(false);
    expect(screen.queryByText(/not answered yet/)).toBeNull();
    expect(send().getAttribute("aria-describedby")).toBeNull();
    m.agent.stop();
  });

  it("one click sends one action: the Choices' name, surface and id, and the answers in question order", async () => {
    const sendAction = vi.fn();
    const m = await mountChoices({ send: sendAction });
    click(radio("Which database?", "Postgres"));
    click(radio("Which login?", "No login"));
    click(checkbox("Docker Compose"));
    click(checkbox("Kubernetes"));
    expect(sendAction).not.toHaveBeenCalled();
    click(send());
    expect(sendAction).toHaveBeenCalledTimes(1);
    expect(sendAction).toHaveBeenCalledWith({
      name: "answer",
      surfaceId: "s1",
      sourceComponentId: "pick",
      // the values of a question are in the order of its options, not of the clicks
      context: {
        answers: [
          { id: "db", values: ["pg"] },
          { id: "auth", values: ["none"] },
          { id: "deploy", values: ["k8s", "compose"] },
        ],
      },
    });
    m.agent.stop();
  });

  it("the action's name and the button's label are the component's", async () => {
    const sendAction = vi.fn();
    const m = await mountChoices(
      { send: sendAction },
      { submitLabel: "Decide", action: { event: { name: "decided" } }, title: "Quick choices" },
    );
    expect(screen.getByRole("heading", { name: "Quick choices" })).toBeTruthy();
    click(radio("Which database?", "SQLite"));
    click(radio("Which login?", "Keycloak"));
    click(screen.getByRole("button", { name: "Decide" }));
    expect(sendAction.mock.calls[0]?.[0]).toMatchObject({ name: "decided" });
    m.agent.stop();
  });

  it("a radio choice replaces the previous one; a checkbox can be taken back", async () => {
    const m = await mountChoices();
    click(radio("Which database?", "Postgres"));
    click(radio("Which database?", "SQLite"));
    expect(radio("Which database?", "Postgres").checked).toBe(false);
    expect(radio("Which database?", "SQLite").checked).toBe(true);
    click(checkbox("Kubernetes"));
    expect(checkbox("Kubernetes").checked).toBe(true);
    click(checkbox("Kubernetes"));
    expect(checkbox("Kubernetes").checked).toBe(false);
    m.agent.stop();
  });

  it("'Other': typing turns it on and replaces the radio choice; its words are sent as `other`, and not once it is off", async () => {
    const sendAction = vi.fn();
    const m = await mountChoices({ send: sendAction });
    click(radio("Which login?", "Keycloak"));
    click(radio("Which database?", "Postgres"));
    const other = screen.getByLabelText("Your own answer to: Which database?") as HTMLInputElement;
    // an empty 'Other' answers nothing
    click(radio("Which database?", "Other"));
    expect(radio("Which database?", "Postgres").checked).toBe(false);
    expect(send().disabled).toBe(true);
    fireEvent.change(other, { target: { value: "  Cockroach  " } });
    expect(send().disabled).toBe(false);
    click(send());
    expect(sendAction.mock.calls[0]?.[0].context.answers[0]).toEqual({
      id: "db",
      values: [],
      other: "Cockroach",
    });
    // typing with another option chosen takes the choice over
    click(radio("Which database?", "SQLite"));
    expect(within(group("Which database?")).getByRole("radio", { name: "Other" })).toHaveProperty(
      "checked",
      false,
    );
    fireEvent.change(other, { target: { value: "Cockroach again" } });
    expect(radio("Which database?", "SQLite").checked).toBe(false);
    expect(
      (within(group("Which database?")).getByRole("radio", { name: "Other" }) as HTMLInputElement)
        .checked,
    ).toBe(true);
    // turned off again: the words stay in the box and are not sent
    click(radio("Which database?", "Postgres"));
    sendAction.mockClear();
    click(send());
    expect(sendAction.mock.calls[0]?.[0].context.answers[0]).toEqual({ id: "db", values: ["pg"] });
    m.agent.stop();
  });

  it("'Other' of a multiple question goes with the boxes, and is at most 500 characters", async () => {
    const sendAction = vi.fn();
    const m = await mountChoices({ send: sendAction });
    click(radio("Which database?", "Postgres"));
    click(radio("Which login?", "Keycloak"));
    click(checkbox("Kubernetes"));
    const own = screen.getByLabelText("Your own answer to: Where does it run?") as HTMLInputElement;
    expect(own.maxLength).toBe(500);
    fireEvent.change(own, { target: { value: "bare metal" } });
    expect(
      (
        within(screen.getByRole("group", { name: /Where does it run\?/ })).getByRole("checkbox", {
          name: "Other",
        }) as HTMLInputElement
      ).checked,
    ).toBe(true);
    click(send());
    expect(sendAction.mock.calls[0]?.[0].context.answers[2]).toEqual({
      id: "deploy",
      values: ["k8s"],
      other: "bare metal",
    });
    m.agent.stop();
  });

  it("a Choices that is not required skips its optional question: an empty answer, in its place", async () => {
    const sendAction = vi.fn();
    const m = await mountChoices({ send: sendAction });
    click(radio("Which database?", "Postgres"));
    click(radio("Which login?", "Keycloak"));
    click(send());
    expect(sendAction.mock.calls[0]?.[0].context.answers[2]).toEqual({ id: "deploy", values: [] });
    m.agent.stop();
  });

  it("is inert when the thread does not wait for the owner: the choices stay, nothing takes input or sends", async () => {
    const sendAction = vi.fn();
    const m = mountSurfaces({ send: sendAction, canSend: false, state: "working" }, {}, undefined, {
      catalogVersion: OWN_CATALOG.version,
    });
    await feed(m, surfaceRun([choicesSurface()]));
    expect(send().disabled).toBe(true);
    // a control inside a disabled fieldset is disabled (jsdom says so through :disabled)
    for (const r of [...screen.getAllByRole("radio"), ...screen.getAllByRole("checkbox")]) {
      expect(r.matches(":disabled")).toBe(true);
    }
    expect(screen.getByLabelText("Your own answer to: Which database?").matches(":disabled")).toBe(
      true,
    );
    // the same note a button gets
    expect(
      screen.getByText(/The actions of this interface work while the thread waits for you/),
    ).toBeTruthy();
    click(send());
    expect(sendAction).not.toHaveBeenCalled();
    m.agent.stop();
  });

  it("only the newest copy of a surface takes answers: the earlier one keeps its choices and loses its controls' power", async () => {
    const sendAction = vi.fn();
    const m = mountSurfaces({ send: sendAction }, {}, undefined, {
      catalogVersion: OWN_CATALOG.version,
    });
    await feed(m, [
      ...surfaceRun([choicesSurface()], { end: "success" }),
      ...surfaceRun([choicesSurface({ title: "Second copy" })], {
        runId: "run-2",
        end: "interrupt",
        user: false,
      }),
    ]);
    // the second run renders after the first: wait for it before judging the first copy
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Second copy" })).toBeTruthy();
      expect(screen.getByText("This interface was updated further down.")).toBeTruthy();
    });
    // one live copy: the earlier is a note, with no radio buttons
    expect(screen.getAllByRole("radiogroup")).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: "Send answers" })).toHaveLength(1);
    expect(screen.getByRole("heading", { name: "Second copy" })).toBeTruthy();
    m.agent.stop();
  });
});

describe("the person's answer in the transcript", () => {
  const answered = (answers: unknown[], over: Rec = {}) => ({
    surfaceId: "s1",
    name: "answer",
    sourceComponentId: "pick",
    context: { answers },
    ...over,
  });

  async function transcript(content: Rec, surfaceOps: unknown[] | null = choicesSurface()) {
    const m = mountSurfaces({ canSend: false, state: "done" }, {}, undefined, {
      catalogVersion: OWN_CATALOG.version,
    });
    const frames = [
      ...(surfaceOps ? surfaceRun([surfaceOps], { end: "interrupt" }) : []),
      ...actionRun(content),
    ];
    await act(async () => {
      m.stream.frames(frames);
    });
    const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
    await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
    await screen.findByText("Going on.");
    return m;
  }
  const bubble = () => document.querySelector('[data-slot="answer-bubble"]') as HTMLElement;

  it("is 'Your answers': each question with the labels the person chose, not a step", async () => {
    const m = await transcript(
      answered([
        { id: "db", values: ["pg"] },
        { id: "auth", values: [], other: "LDAP" },
        { id: "deploy", values: ["k8s", "compose"] },
      ]),
    );
    expect(bubble()).not.toBeNull();
    expect(within(bubble()).getByText("Your answers")).toBeTruthy();
    const lines = Array.from(bubble().querySelectorAll('[data-slot="answer-line"]')).map((l) => [
      l.querySelector("dt")?.textContent,
      l.querySelector("dd")?.textContent,
    ]);
    expect(lines).toEqual([
      ["Which database?", "Postgres"],
      ["Which login?", "Other: LDAP"],
      ["Where does it run?", "Kubernetes, Docker Compose"],
    ]);
    // the person's, so it is not "Chose answer" in the step list
    expect(document.querySelector('[data-slot="action-step"]')).toBeNull();
    expect(screen.queryByText(/Chose/)).toBeNull();
    m.agent.stop();
  });

  it("sits above the agent's mark, right-aligned like the person's words", async () => {
    const m = await transcript(answered([{ id: "db", values: ["pg"] }]));
    const turn = bubble().closest('[data-slot="agent-turn"]') as HTMLElement;
    expect(turn).not.toBeNull();
    const mark = turn.querySelector('[data-slot="actor-label"]') as HTMLElement;
    // the bubble comes before the agent's label in the document
    expect(bubble().compareDocumentPosition(mark) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(bubble().className).toContain("self-end");
    expect(bubble().className).toContain("bg-bubble");
    m.agent.stop();
  });

  it("a skipped optional question reads 'No answer'", async () => {
    const m = await transcript(
      answered([
        { id: "db", values: ["sqlite"] },
        { id: "deploy", values: [] },
      ]),
    );
    const lines = Array.from(bubble().querySelectorAll('[data-slot="answer-line"]')).map(
      (l) => l.querySelector("dd")?.textContent,
    );
    expect(lines).toEqual(["SQLite", "No answer"]);
    m.agent.stop();
  });

  it("shows the raw ids and values when the surface is not in the transcript, or does not have them", async () => {
    const gone = await transcript(answered([{ id: "db", values: ["pg"] }]), null);
    expect(bubble().querySelector("dt")?.textContent).toBe("db");
    expect(bubble().querySelector("dd")?.textContent).toBe("pg");
    gone.agent.stop();
    cleanup();
    resetSeq();
    const unknown = await transcript(
      answered([
        { id: "db", values: ["mongo"] },
        { id: "zzz", values: ["q"] },
      ]),
    );
    const lines = Array.from(bubble().querySelectorAll('[data-slot="answer-line"]')).map((l) => [
      l.querySelector("dt")?.textContent,
      l.querySelector("dd")?.textContent,
    ]);
    expect(lines).toEqual([
      ["Which database?", "mongo"],
      ["zzz", "q"],
    ]);
    unknown.agent.stop();
  });

  it("is text: markup in the agent's labels and the person's words stays characters", async () => {
    const m = await transcript(
      answered([{ id: "db", values: [], other: "<img src=x onerror=alert(1)>" }]),
    );
    expect(bubble().textContent).toContain("<img src=x onerror=alert(1)>");
    expect(bubble().querySelector("img, script, a")).toBeNull();
    m.agent.stop();
  });

  it("a button's action is still a step, and so is an action whose context is not an answer", async () => {
    const button = await transcript(
      { surfaceId: "s1", name: "go", sourceComponentId: "go", context: { choice: "a" } },
      null,
    );
    expect(bubble()).toBeNull();
    expect(document.querySelector('[data-slot="action-step"]')?.textContent).toContain("Chose go");
    button.agent.stop();
    cleanup();
    resetSeq();
    const odd = await transcript(
      { surfaceId: "s1", name: "answer", sourceComponentId: "pick", context: { answers: "x" } },
      null,
    );
    expect(bubble()).toBeNull();
    expect(document.querySelector('[data-slot="action-step"]')).not.toBeNull();
    odd.agent.stop();
  });

  it("a Choices that was answered is read-only where it stands: the thread moved on", async () => {
    const sendAction = vi.fn();
    const m = mountSurfaces({ send: sendAction, canSend: false, state: "done" }, {}, undefined, {
      catalogVersion: OWN_CATALOG.version,
    });
    const frames = [
      ...surfaceRun([choicesSurface()], { end: "interrupt" }),
      ...actionRun(answered([{ id: "db", values: ["pg"] }])),
    ];
    await act(async () => {
      m.stream.frames(frames);
    });
    await screen.findByText("Going on.");
    for (const r of screen.getAllByRole("radio")) expect(r.matches(":disabled")).toBe(true);
    expect(send().disabled).toBe(true);
    m.agent.stop();
  });
});
