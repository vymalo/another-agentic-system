// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TurnSteps } from "@/features/chat/lib/step-tree";
import { type ExpansionState, NO_EXPANSION } from "./expansion";
import { type StepsFocus, StepsPane } from "./steps-pane";
import { actorPart, assistant, statusPart, stepPart, turnsOf } from "./testing";

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

function Harness({
  turns,
  focus = null,
}: {
  turns: readonly TurnSteps[];
  focus?: StepsFocus | null;
}) {
  const [expanded, setExpanded] = useState<ExpansionState>(NO_EXPANSION);
  return (
    <StepsPane
      turns={turns}
      focus={focus}
      live={false}
      expanded={expanded}
      onExpandedChange={setExpanded}
    />
  );
}

const SEARCH = {
  kind: "tool",
  label: "search__web_search",
  icon: "search",
  input: { query: "Stephane Segning", limit: 3, api_key: "[redacted]" },
};
const SEARCH_END = {
  ...SEARCH,
  output: { text: "1. Stephane Segning - vymalo\n2. Another result" },
};

/** A turn with these steps, each as its start and its end. */
const turnOf = (steps: [string, string, Record<string, unknown>][]): TurnSteps[] =>
  turnsOf([
    assistant([
      actorPart(),
      statusPart("working", undefined, 1),
      ...steps.map(([id, state, extra], i) => stepPart(id, state, extra, 2 + i)),
    ]),
  ]);

const rowOf = (id: string): HTMLElement =>
  document.querySelector(`[data-step="${id}"]`) as HTMLElement;
const toggleOf = (id: string): HTMLElement =>
  rowOf(id).querySelector('[data-slot="step-toggle"]') as HTMLElement;
const blockOf = (id: string): HTMLElement | null =>
  rowOf(id).querySelector('[data-slot="step-io"]');

describe("a tool step with input or output opens onto them", () => {
  it("is called by its tool, from its server, with what it was asked", () => {
    render(<Harness turns={turnOf([["T/s1", "completed", SEARCH_END]])} />);
    const toggle = toggleOf("T/s1");
    expect(toggle.textContent).toBe("Web searchfrom searchStephane Segning");
    // the raw label stays in the tooltip, the server is said to a screen reader
    expect(toggle.querySelector("span[title]")?.getAttribute("title")).toBe("search__web_search");
    expect(within(toggle).getByText("from")).toBeTruthy();
    expect(screen.queryByText("search__web_search")).toBeNull();
  });

  it("is a native button with aria-expanded, closed until it is opened, and opens by a click", () => {
    render(<Harness turns={turnOf([["T/s1", "completed", SEARCH_END]])} />);
    const toggle = toggleOf("T/s1");
    expect(toggle.tagName).toBe("BUTTON");
    expect(toggle.getAttribute("type")).toBe("button");
    expect(toggle.tabIndex).toBe(0);
    toggle.focus();
    expect(document.activeElement).toBe(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.hasAttribute("aria-controls")).toBe(false);
    expect(blockOf("T/s1")).toBeNull();

    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    const block = blockOf("T/s1") as HTMLElement;
    expect(toggle.getAttribute("aria-controls")).toBe(block.id);

    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(blockOf("T/s1")).toBeNull();
  });

  it("shows Input, then Output: the arguments as a list, the result as monospace text", () => {
    render(<Harness turns={turnOf([["T/s1", "completed", SEARCH_END]])} />);
    fireEvent.click(toggleOf("T/s1"));
    const block = blockOf("T/s1") as HTMLElement;
    const headings = within(block).getAllByRole("heading", { level: 4 });
    expect(headings.map((h) => h.textContent)).toEqual(["Input", "Output"]);

    const input = block.querySelector('[data-slot="step-input"]') as HTMLElement;
    expect(input.tagName).toBe("DL");
    expect(within(input).getByText("query").nextElementSibling?.textContent).toBe(
      "Stephane Segning",
    );
    expect(within(input).getByText("limit").nextElementSibling?.textContent).toBe("3");
    // a credential was redacted by the orchestrator, and is drawn as it came
    expect(within(input).getByText("api_key").nextElementSibling?.textContent).toBe("[redacted]");

    const output = block.querySelector('[data-slot="step-output"]') as HTMLElement;
    expect(output.tagName).toBe("PRE");
    expect(output.textContent).toBe("1. Stephane Segning - vymalo\n2. Another result");
    expect(output.className).toContain("font-mono");
    expect(output.tabIndex).toBe(0);
    expect(within(block).queryByRole("heading", { name: "Error" })).toBeNull();
    expect(block.querySelector('[data-slot="step-output-cut"]')).toBeNull();
  });

  it("shows nested arguments as pretty JSON text", () => {
    const input = { path: "a.txt", edits: [{ from: "x", to: "y" }], opts: { force: true } };
    render(<Harness turns={turnOf([["T/e", "completed", { label: "files__edit", input }]])} />);
    fireEvent.click(toggleOf("T/e"));
    const shown = (blockOf("T/e") as HTMLElement).querySelector(
      '[data-slot="step-input"]',
    ) as HTMLElement;
    expect(shown.tagName).toBe("PRE");
    expect(JSON.parse(shown.textContent ?? "")).toEqual(input);
    expect(shown.textContent).toContain('\n  "path": "a.txt"');
  });

  it("says an input that was too big to keep, and how big", () => {
    const cut = { label: "files__write_many", input: { _cut: true, bytes: 18432 } };
    render(<Harness turns={turnOf([["T/c", "completed", cut]])} />);
    fireEvent.click(toggleOf("T/c"));
    const block = blockOf("T/c") as HTMLElement;
    expect(within(block).getByText("Input not kept (18 KiB)")).toBeTruthy();
    expect(block.querySelector('[data-slot="step-input"]')).toBeNull();
    // there is no argument to quote on the row
    expect(rowOf("T/c").querySelector('[data-slot="step-preview"]')).toBeNull();
  });

  it("says how much of a cut output is not there", () => {
    const text = `${"head ".repeat(20)}\n… 41808 bytes not kept …\n${"tail ".repeat(20)}`;
    const output = { text, truncated: true, bytes: 50_000 };
    render(<Harness turns={turnOf([["T/f", "completed", { label: "fetch__page", output }]])} />);
    fireEvent.click(toggleOf("T/f"));
    const block = blockOf("T/f") as HTMLElement;
    const left = 50_000 - new TextEncoder().encode(text).length;
    expect(left).toBeGreaterThan(40_000);
    expect(within(block).getByText(/^\d+ KiB more not kept$/)).toBeTruthy();
    expect(block.querySelector('[data-slot="step-output"]')?.textContent).toBe(text);
  });

  it("shows Error, in place of Output, when the call failed", () => {
    const failed = {
      kind: "command",
      label: "npm run build",
      detail: "exit 1",
      input: { command: "npm run build", cwd: "web" },
      output: { text: "Failed to compile.\nType error: nope", error: true },
    };
    render(<Harness turns={turnOf([["T/b", "failed", failed]])} />);
    fireEvent.click(toggleOf("T/b"));
    const block = blockOf("T/b") as HTMLElement;
    expect(
      within(block)
        .getAllByRole("heading", { level: 4 })
        .map((h) => h.textContent),
    ).toEqual(["Input", "Error"]);
    expect(block.querySelector('[data-slot="step-output"]')?.textContent).toBe(
      "Failed to compile.\nType error: nope",
    );
    expect(within(block).queryByRole("heading", { name: "Output" })).toBeNull();
  });

  it("shows Error for a failed step whose output was not marked, and for one that has only a detail", () => {
    const plain = { label: "x__y", output: { text: "it broke" } };
    render(<Harness turns={turnOf([["T/p", "failed", plain]])} />);
    fireEvent.click(toggleOf("T/p"));
    expect(
      within(blockOf("T/p") as HTMLElement).getByRole("heading", { name: "Error" }),
    ).toBeTruthy();
    cleanup();

    render(
      <Harness
        turns={turnOf([["T/d", "failed", { label: "x__y", detail: "timed out", input: { a: 1 } }]])}
      />,
    );
    fireEvent.click(toggleOf("T/d"));
    const block = blockOf("T/d") as HTMLElement;
    expect(within(block).getByRole("heading", { name: "Error" })).toBeTruthy();
    expect(within(block).getAllByText("timed out").length).toBeGreaterThan(0);
  });

  it("notes what the job's record limit left out", () => {
    render(
      <Harness
        turns={turnOf([["T/n", "completed", { label: "search__web_search", ioDropped: true }]])}
      />,
    );
    expect(toggleOf("T/n").getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggleOf("T/n"));
    const block = blockOf("T/n") as HTMLElement;
    expect(block.querySelector('[data-slot="step-io-dropped"]')?.textContent).toMatch(
      /not kept: the job passed its recording limit/,
    );
    expect(within(block).queryByRole("heading", { name: "Input" })).toBeNull();
  });

  it("is not a control when it has nothing to open", () => {
    render(<Harness turns={turnOf([["T/none", "completed", { label: "no_details" }]])} />);
    expect(rowOf("T/none").querySelector("button")).toBeNull();
    expect(rowOf("T/none").textContent).toContain("no_details");
  });

  it("draws everything as text: markup in an argument, a result or an error is shown, never run", () => {
    const evil = '<img src=x onerror="alert(1)"><script>alert(2)</script>';
    render(
      <Harness
        turns={turnOf([
          [
            "T/h",
            "failed",
            {
              label: "evil__tool",
              input: { query: evil, deep: { html: evil } },
              output: { text: `${evil}\n<b>bold</b>`, error: true },
            },
          ],
        ])}
      />,
    );
    // the row's own preview is text as well
    expect(rowOf("T/h").textContent).toContain("<img src=x");
    fireEvent.click(toggleOf("T/h"));
    const block = blockOf("T/h") as HTMLElement;
    expect(block.querySelector("img, script, b, a, iframe")).toBeNull();
    expect(block.textContent).toContain(evil);
    expect(block.textContent).toContain("<b>bold</b>");
    expect(document.querySelector("img[src='x'], script")).toBeNull();
    // and a flat list: a value is a text node too
    cleanup();
    render(
      <Harness turns={turnOf([["T/h2", "completed", { label: "e__t", input: { q: evil } }]])} />,
    );
    fireEvent.click(toggleOf("T/h2"));
    expect((blockOf("T/h2") as HTMLElement).querySelector("img, script")).toBeNull();
    expect((blockOf("T/h2") as HTMLElement).textContent).toContain(evil);
  });

  it("a step with children opens its children and its own input and output with one button", () => {
    const turns = turnsOf([
      assistant([
        actorPart(),
        stepPart(
          "T/oc",
          "completed",
          {
            kind: "subagent",
            label: "OpenCode",
            input: { task: "fix it" },
            output: { text: "Done." },
          },
          2,
        ),
        stepPart("T/c", "completed", { label: "child" }, 3, ["T/oc"]),
      ]),
    ]);
    render(<Harness turns={turns} />);
    const toggle = toggleOf("T/oc");
    expect(toggle.textContent).toBe("OpenCode· 1 step");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(blockOf("T/oc")).not.toBeNull();
    expect(screen.getByRole("list", { name: "Steps of OpenCode" })).toBeTruthy();
    expect(toggle.getAttribute("aria-controls")?.split(" ")).toHaveLength(2);
    fireEvent.click(toggle);
    expect(blockOf("T/oc")).toBeNull();
    expect(screen.queryByRole("list", { name: "Steps of OpenCode" })).toBeNull();
  });
});

describe("a request to show a step", () => {
  it("opens the way to it and the step, and puts the focus on its button", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "requestAnimationFrame", "cancelAnimationFrame"] });
    try {
      const turns = turnsOf([
        assistant(
          [
            actorPart(),
            stepPart("T/oc", "completed", { kind: "subagent", label: "OpenCode" }, 2),
            ...Array.from({ length: 6 }, (_, i) =>
              stepPart(`T/k${i}`, "completed", { label: `k${i}` }, 3 + i, ["T/oc"]),
            ),
            stepPart(
              "T/bad",
              "failed",
              {
                kind: "command",
                label: "npm test",
                detail: "1 failed",
                output: { text: "FAIL src/a.test.ts", error: true },
              },
              9,
              ["T/oc"],
            ),
          ],
          { type: "complete" },
          "t1",
        ),
      ]);
      render(<Harness turns={turns} focus={{ turnId: "t1", key: 1, stepId: "T/bad" }} />);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(100);
      });
      expect(toggleOf("T/oc").getAttribute("aria-expanded")).toBe("true");
      expect(toggleOf("T/bad").getAttribute("aria-expanded")).toBe("true");
      expect(blockOf("T/bad")?.textContent).toContain("FAIL src/a.test.ts");
      expect(document.activeElement).toBe(toggleOf("T/bad"));
    } finally {
      vi.useRealTimers();
    }
  });

  it("settles for the turn's header when the step is not there", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "requestAnimationFrame", "cancelAnimationFrame"] });
    try {
      const turns = turnsOf([
        assistant([actorPart(), stepPart("T/a", "completed", {}, 2)], undefined, "t1"),
      ]);
      render(<Harness turns={turns} focus={{ turnId: "t1", key: 1, stepId: "T/gone" }} />);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(200);
      });
      expect(document.activeElement).toBe(screen.getByRole("heading", { level: 3 }));
    } finally {
      vi.useRealTimers();
    }
  });
});
