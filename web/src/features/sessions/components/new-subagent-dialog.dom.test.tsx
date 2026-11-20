// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { type ReactNode, useRef, useState } from "react";
import { afterAll, afterEach, beforeAll, describe, expect, it, type Mock, vi } from "vitest";
import { DEFAULT_OFFER } from "../../../../mock/sessions";
import type { StartDraft } from "../hooks/use-start-session";
import { type ChildSessionsOffer, parseChildSessionsOffer } from "../lib/offer";
import { type MockPage, mockPage } from "../test-mock";

/*
 * "New subagent", against the mock orchestrator (ADR 0072): the helpers of the live card, the
 * description counted in code points, "Close when done" on by default and sent the other way round as
 * `keepOpen`, a 202 that closes the dialog and hands the draft to its caller, every refusal in place with
 * the draft kept, and the keyboard.
 */

let page: MockPage;
let NewSubagentDialog: typeof import("./new-subagent-dialog").NewSubagentDialog;
const offer = parseChildSessionsOffer(DEFAULT_OFFER) as ChildSessionsOffer;

beforeAll(async () => {
  page = await mockPage();
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  ({ NewSubagentDialog } = await import("./new-subagent-dialog"));
});
afterAll(() => page.restore());
afterEach(() => {
  cleanup();
  page.fake("POST /api/threads/:id/sessions", null);
});

type Spies = {
  onStarted: Mock<(request: string, draft: StartDraft) => void>;
  onOpened: Mock<() => void>;
  onCardChanged: Mock<() => void>;
  onWithdrawn: Mock<() => void>;
  onOpenChange: Mock<(open: boolean) => void>;
};

function mount(
  parent: string,
  over: {
    offer?: ChildSessionsOffer | null;
    tier?: "plan" | "edits" | "auto" | "bypass";
    initial?: StartDraft;
  } = {},
): Spies {
  const spies: Spies = {
    onStarted: vi.fn(),
    onOpened: vi.fn(),
    onCardChanged: vi.fn(),
    onWithdrawn: vi.fn(),
    onOpenChange: vi.fn(),
  };
  const returnTo = { current: null };
  render(
    <NewSubagentDialog
      open
      parent={parent}
      offer={over.offer === undefined ? offer : over.offer}
      tier={over.tier}
      initial={over.initial}
      returnTo={returnTo}
      {...spies}
    />,
  );
  return spies;
}

const fill = (description: string, text = "Plan it") => {
  fireEvent.change(screen.getByLabelText("What it is for"), { target: { value: description } });
  fireEvent.change(screen.getByLabelText("What to ask it"), { target: { value: text } });
};
const submit = () => fireEvent.click(screen.getByRole("button", { name: "Start subagent" }));
/** The `child_session_start_asked` events of the parent. */
const asked = async (parent: string) => page.api.of(parent, "child_session_start_asked");

describe("the dialog", () => {
  it("reads the card again when it opens, lists the helpers with their own descriptions as text, and picks the first", () => {
    const spies = mount("00000000-0000-4000-8000-0000000000aa", {
      offer: {
        agents: [
          { name: "planner", description: "<b>Breaks</b> **it** down." },
          ...offer.agents.slice(1),
        ],
        maxOpen: 4,
      },
    });
    expect(spies.onOpened).toHaveBeenCalledTimes(1);
    const dialog = screen.getByRole("dialog", { name: "New subagent" });
    const radios = within(dialog).getAllByRole("radio");
    expect(radios.map((r) => r.getAttribute("value"))).toEqual(["planner", "researcher"]);
    expect(radios[0]?.getAttribute("aria-checked")).toBe("true");
    expect(dialog.textContent).toContain("<b>Breaks</b> **it** down.");
    expect(dialog.querySelector("b, strong")).toBeNull();
    expect(dialog.textContent).toContain("Looks things up on the web.");
  });

  it("is a select, with the chosen helper's description beneath, when the card lists many", () => {
    const many = {
      maxOpen: 8,
      agents: ["a", "b", "c", "d", "e"].map((n) => ({ name: n, description: `does ${n}` })),
    };
    mount("00000000-0000-4000-8000-0000000000aa", { offer: many });
    const select = screen.getByRole("combobox", { name: "Subagent" });
    expect(screen.queryAllByRole("radio")).toHaveLength(0);
    expect(screen.getByText("does a")).toBeTruthy();
    fireEvent.change(select, { target: { value: "c" } });
    expect(screen.getByText("does c")).toBeTruthy();
  });

  it("has 'Close when done' on by default, and sends it as keepOpen: false, with the 202 closing the dialog", async () => {
    const parent = await page.api.answered();
    // the card of `adam` lists the tiers: a start that names one is checked against it
    await page.api.json("POST", `/__mock/tiers?session=${page.who.name}`, {
      agent: "adam",
      params: { tiers: ["plan", "edits", "auto"], default: "auto" },
    });
    const spies = mount(parent, { tier: "plan" });
    const box = screen.getByRole("checkbox", { name: /Close when done/ }) as HTMLInputElement;
    expect(box.checked).toBe(true);
    fill("Plan the release", "Go.");
    submit();
    await waitFor(() => expect(spies.onStarted).toHaveBeenCalledTimes(1));
    const [request, draft] = spies.onStarted.mock.calls[0] as [string, StartDraft];
    expect(draft).toEqual({
      agent: "planner",
      description: "Plan the release",
      text: "Go.",
      keepOpen: false,
    });
    expect(spies.onOpenChange).toHaveBeenCalledWith(false);
    const [event] = await asked(parent);
    expect(event?.data).toMatchObject({ request, agent: "planner", keepOpen: false, tier: "plan" });
  });

  it("sends keepOpen: true when the box is cleared, and the helper that was picked", async () => {
    const parent = await page.api.answered();
    const spies = mount(parent);
    fireEvent.click(screen.getByRole("radio", { name: /researcher/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /Close when done/ }));
    fill("Check the sources");
    submit();
    await waitFor(() => expect(spies.onStarted).toHaveBeenCalled());
    const [event] = await asked(parent);
    expect(event?.data).toMatchObject({ agent: "researcher", keepOpen: true });
    expect(event?.data).not.toHaveProperty("tier");
  });

  it("makes one request id for the dialog and sends the same one on a repeat", async () => {
    const parent = await page.api.answered();
    mount(parent);
    fill("Plan it");
    page.fake("POST /api/threads/:id/sessions", { status: 503, body: { title: "Unavailable" } });
    submit();
    await screen.findByRole("alert");
    page.fake("POST /api/threads/:id/sessions", null);
    submit();
    await waitFor(() => expect(page.seen.at(-1)).toMatch(/sessions 202$/));
    const events = await asked(parent);
    expect(events).toHaveLength(1);
  });
});

describe("the description", () => {
  const COUNT = () => screen.getByTestId("description-count").textContent;

  it("counts code points: an emoji is one, so 79 letters and an emoji are 80/80 and are sent", async () => {
    const parent = await page.api.answered();
    const spies = mount(parent);
    const eighty = `${"a".repeat(79)}😀`;
    expect(eighty.length).toBe(81); // UTF-16 units: the rule is not this one
    fill(eighty);
    expect(COUNT()).toBe("80/80");
    submit();
    await waitFor(() => expect(spies.onStarted).toHaveBeenCalled());
    expect((await asked(parent))[0]?.data.description).toBe(eighty);
  });

  it("is 81/80 with one more, refused on screen before any request, and nothing is lost", async () => {
    const parent = await page.api.answered();
    const spies = mount(parent);
    const eighty1 = `${"a".repeat(80)}😀`;
    fill(eighty1, "Keep this prompt");
    expect(COUNT()).toBe("81/80");
    const before = page.seen.length;
    submit();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("81 characters; at most 80");
    expect(page.seen.length).toBe(before);
    expect(spies.onStarted).not.toHaveBeenCalled();
    expect((screen.getByLabelText("What it is for") as HTMLInputElement).value).toBe(eighty1);
    expect((screen.getByLabelText("What to ask it") as HTMLTextAreaElement).value).toBe(
      "Keep this prompt",
    );
    expect(document.activeElement).toBe(screen.getByLabelText("What it is for"));
  });

  it("is blank or without a prompt: told in place, nothing sent", () => {
    mount("00000000-0000-4000-8000-0000000000aa");
    fill("   ", "");
    submit();
    expect(screen.getAllByRole("alert").map((a) => a.textContent)).toEqual([
      "Describe the subagent in one line of 1 to 80 characters.",
      "Write what the subagent should do.",
    ]);
    fill("Fine", "");
    submit();
    expect(screen.getByRole("alert").textContent).toBe("Write what the subagent should do.");
  });
});

describe("a refusal stays in the dialog with the draft", () => {
  async function refused(parent: string, code: string, status: number, detail = "no") {
    const spies = mount(parent);
    fill("Plan the docs", "My long prompt");
    page.fake("POST /api/threads/:id/sessions", { status, body: { title: "x", detail, code } });
    submit();
    await waitFor(() => expect(page.seen.at(-1)).toMatch(new RegExp(` ${status}$`)));
    expect(spies.onStarted).not.toHaveBeenCalled();
    expect(spies.onOpenChange).not.toHaveBeenCalled();
    expect((screen.getByLabelText("What it is for") as HTMLInputElement).value).toBe(
      "Plan the docs",
    );
    expect((screen.getByLabelText("What to ask it") as HTMLTextAreaElement).value).toBe(
      "My long prompt",
    );
    return spies;
  }

  it("no_context, at the top, in the screen's words", async () => {
    await refused("00000000-0000-4000-8000-0000000000aa", "no_context", 409);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Send the agent a message first");
  });

  it("too_many_starts: four starts waiting (the mock, held), at the top", async () => {
    const parent = await page.api.answered();
    await page.api.config("sessionStarts=hold");
    for (let i = 0; i < 4; i++) expect((await page.api.start(parent)).status).toBe(202);
    const spies = mount(parent);
    fill("Another one");
    submit();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Wait for the subagents you started to open");
    expect(spies.onStarted).not.toHaveBeenCalled();
    expect((screen.getByLabelText("What it is for") as HTMLInputElement).value).toBe("Another one");
    await page.api.config("sessionStarts=open");
  });

  it("too_many_sessions: the thread's limit (the mock's), at the top", async () => {
    const parent = await page.api.answered();
    await page.api.config("sessionsNoted=1");
    await page.api.opened(parent);
    mount(parent);
    fill("One too many");
    submit();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("reached the most subagents it can keep");
    await page.api.config("sessionsNoted=256");
  });

  it("unknown_helper, on the helper field, and the card is read again", async () => {
    const parent = await page.api.answered();
    const spies = mount(parent, {
      offer: { maxOpen: 4, agents: [{ name: "ghost", description: "Not on the card." }] },
    });
    fill("Haunt");
    submit();
    const dialog = screen.getByRole("dialog");
    await waitFor(() =>
      expect(within(dialog).getByText(/no subagent with that name/i)).toBeTruthy(),
    );
    expect(within(dialog).getByRole("group", { name: "Subagent" }).textContent).toContain(
      "This agent has no subagent with that name.",
    );
    expect(spies.onCardChanged).toHaveBeenCalledTimes(1);
  });

  it("bad_description, on the description field, which is marked invalid and focused", async () => {
    await refused("00000000-0000-4000-8000-0000000000aa", "bad_description", 422);
    const field = screen.getByLabelText("What it is for") as HTMLInputElement;
    await waitFor(() => expect(field.getAttribute("aria-invalid")).toBe("true"));
    expect(document.activeElement).toBe(field);
    expect(screen.getByRole("alert").textContent).toContain("1 to 80 characters");
    // the field says why, and it is the field's own message
    expect(field.getAttribute("aria-describedby")).toContain(screen.getByRole("alert").id);
  });

  it("no_child_sessions (the card says so now): in words, the card is read again and the button is withdrawn", async () => {
    const parent = await page.api.answered();
    const spies = mount(parent);
    await page.api.config("sessions=off");
    fill("Too late");
    submit();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("does not offer subagents right now");
    expect(spies.onWithdrawn).toHaveBeenCalledTimes(1);
    await page.api.config("sessions=on");
  });

  it("says in the same dialog that nothing is offered once the card is gone, with no way to start", () => {
    mount("00000000-0000-4000-8000-0000000000aa", { offer: null });
    expect(screen.getByRole("status").textContent).toContain("does not offer subagents right now");
    expect(
      (screen.getByRole("button", { name: "Start subagent" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(screen.queryByLabelText("What it is for")).toBeNull();
  });
});

describe("a retry after a start that failed", () => {
  it("opens with the draft, and starts it again as a new request", async () => {
    const parent = await page.api.answered();
    const spies = mount(parent, {
      initial: {
        agent: "researcher",
        description: "Build the crate",
        text: "cargo build",
        keepOpen: true,
      },
    });
    expect((screen.getByLabelText("What it is for") as HTMLInputElement).value).toBe(
      "Build the crate",
    );
    expect((screen.getByLabelText("What to ask it") as HTMLTextAreaElement).value).toBe(
      "cargo build",
    );
    expect(
      (screen.getByRole("checkbox", { name: /Close when done/ }) as HTMLInputElement).checked,
    ).toBe(false);
    expect(screen.getByRole("radio", { name: /researcher/ }).getAttribute("aria-checked")).toBe(
      "true",
    );
    submit();
    await waitFor(() => expect(spies.onStarted).toHaveBeenCalled());
    expect((await asked(parent))[0]?.data).toMatchObject({ agent: "researcher", keepOpen: true });
  });
});

describe("the keyboard and the focus", () => {
  function Opener(): ReactNode {
    const returnTo = useRef<HTMLElement | null>(null);
    const [open, setOpen] = useState(false);
    return (
      <>
        <button
          type="button"
          onClick={(e) => {
            returnTo.current = e.currentTarget;
            setOpen(true);
          }}
        >
          New subagent
        </button>
        <NewSubagentDialog
          open={open}
          onOpenChange={setOpen}
          parent="00000000-0000-4000-8000-0000000000aa"
          offer={offer}
          tier={undefined}
          onOpened={() => {}}
          onCardChanged={() => {}}
          onWithdrawn={() => {}}
          onStarted={() => {}}
          returnTo={returnTo}
        />
      </>
    );
  }

  it("takes the focus in, moves between helpers with the arrows, and Escape gives the focus back to the opener", async () => {
    render(<Opener />);
    const opener = screen.getByRole("button", { name: "New subagent" });
    opener.focus();
    fireEvent.click(opener);
    const dialog = await screen.findByRole("dialog", { name: "New subagent" });
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));

    const planner = screen.getByRole("radio", { name: /planner/ });
    planner.focus();
    fireEvent.keyDown(planner, { key: "ArrowDown" });
    await waitFor(() =>
      expect(screen.getByRole("radio", { name: /researcher/ }).getAttribute("aria-checked")).toBe(
        "true",
      ),
    );

    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });

  it("is a form: every control is a native or Radix control with a name, and Cancel closes it", async () => {
    render(<Opener />);
    fireEvent.click(screen.getByRole("button", { name: "New subagent" }));
    const dialog = await screen.findByRole("dialog");
    for (const control of within(dialog).getAllByRole("textbox")) {
      expect(control.getAttribute("id")).toBeTruthy();
    }
    expect(within(dialog).getByLabelText("What it is for")).toBeTruthy();
    expect(within(dialog).getByLabelText("What to ask it")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
