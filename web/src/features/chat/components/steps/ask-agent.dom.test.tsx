// @vitest-environment jsdom
import { useAuiState } from "@assistant-ui/react";
import {
  act,
  cleanup,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentNamesProvider } from "@/features/agents/components/agent-names-context";
import { framesThrough, LiveStream, loadGolden, sse } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import { StepsPanelTestProvider } from "@/features/panel/hooks/use-steps-panel";
import type { ApiAgent } from "@/lib/api/types";
import { ThreadViewProvider } from "../thread-view";
import { StepsPanelContent } from "./steps-panel-content";
import { TurnSummaryLine } from "./turn-summary";

configure({ asyncUtilTimeout: 10_000 });

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
});
afterEach(cleanup);

const AGENTS = [
  { id: "coder", name: "Coder", source: "static" },
  { id: "researcher", name: "Researcher", source: "static" },
] as ApiAgent[];

/**
 * The `ask-agent` golden (docs/api/examples/agui/ask-agent.agui.json) through the real runtime into
 * the panel's Activity tab, as far as `upTo` (the `id` of a frame; the stream stays open, so the
 * thread is still working): the agent asked `coder`, which asked `researcher` (both answer), and
 * asked `researcher` again, which fails.
 */
async function play(upTo: number | null) {
  const stream = new LiveStream();
  const all = loadGolden("ask-agent");
  const frames = upTo === null ? all : framesThrough(all, upTo);
  const state = upTo === null ? ("done" as const) : ("working" as const);
  const mounted = mountRuntime(
    () => sse(stream.body),
    {},
    <StepsPanelTestProvider>
      <AgentNamesProvider agents={AGENTS}>
        <ThreadViewProvider value={{ state, waiting: false, agentId: "plain" }}>
          <StepsPanelContent />
          <Summary />
        </ThreadViewProvider>
      </AgentNamesProvider>
    </StepsPanelTestProvider>,
  );
  mounted.agent.start();
  await act(async () => {
    stream.frames(frames);
  });
  await waitFor(() => expect(mounted.agent.getSnapshot().lastSeq).toBe(frames.at(-1)?.id));
  if (upTo === null) {
    await waitFor(() => expect(mounted.runtime().thread.getState().isRunning).toBe(false));
  }
  return mounted;
}

/** The chat's one line for the turn: the runtime's only assistant message here. */
function Summary() {
  const id = useAuiState((s) => s.thread.messages.find((m) => m.role === "assistant")?.id);
  return id ? <TurnSummaryLine turnId={id} /> : null;
}

const pane = () => screen.findByRole("region", { name: /^Turn 1/ });
const askOf = (within_: HTMLElement, name: RegExp | string) =>
  within(within_).getByRole("button", { name });
const row = (button: HTMLElement) => button.closest("li") as HTMLElement;
const spins = (el: HTMLElement) => el.querySelector("svg.lucide-loader-circle") !== null;

describe("the ask-agent golden, in the panel", () => {
  it("an ask that runs is one collapsed line, 'Asked Coder', with a spinner and the word Working", async () => {
    const { agent } = await play(3);
    const turn = await pane();
    const coder = askOf(turn, /^Asked Coder/);
    expect(coder.getAttribute("aria-expanded")).toBe("false");
    expect(coder.textContent).toBe("Asked Coder: Working");
    expect(row(coder).getAttribute("data-ask-state")).toBe("running");
    expect(row(coder).getAttribute("data-state")).toBe("live");
    expect(spins(row(coder))).toBe(true);
    // nothing of what it was asked or answered until it is opened
    expect(within(turn).queryByText("coordinate researcher")).toBeNull();
    agent.stop();
  });

  it("opens onto what was asked; the ask coder made is nested under it, running, with its own spinner", async () => {
    const { agent } = await play(4);
    const turn = await pane();
    const coder = askOf(turn, /^Asked Coder/);
    // closed, it says how much is under it
    expect(coder.textContent).toBe("Asked Coder: Working· 1 step");
    fireEvent.click(coder);
    expect(coder.getAttribute("aria-expanded")).toBe("true");
    const details = row(coder).querySelector('[data-slot="ask-details"]') as HTMLElement;
    expect(within(details).getByText("coordinate researcher")).toBeTruthy();
    // the researcher is a line of its own, inside the coder's row
    const researcher = askOf(row(coder), /^Asked Researcher/);
    expect(row(coder).contains(researcher)).toBe(true);
    expect(researcher.textContent).toBe("Asked Researcher: Working");
    expect(spins(row(researcher))).toBe(true);
    expect(coder.getAttribute("aria-controls")).toContain(details.getAttribute("id") ?? "never");
    agent.stop();
  });

  it("both end Answered, with the answer on open; no spinner is left", async () => {
    const { agent } = await play(6);
    const turn = await pane();
    const coder = askOf(turn, /^Asked Coder/);
    expect(coder.textContent).toBe("Asked Coder: Answered· 1 step");
    expect(row(coder).getAttribute("data-state")).toBe("done");
    expect(spins(row(coder))).toBe(false);
    fireEvent.click(coder);
    expect(within(row(coder)).getAllByText(/^coordinate: researcher: completed/)).toHaveLength(1);
    const researcher = askOf(row(coder), /^Asked Researcher/);
    expect(researcher.textContent).toBe("Asked Researcher: Answered");
    fireEvent.click(researcher);
    const answers = within(row(researcher)).getByText("echo: echo researcher");
    expect(answers).toBeTruthy();
    // what it handed back is a name, and a link only for an https URL
    const link = within(row(researcher)).getByRole("link", { name: "result" });
    expect(link.getAttribute("href")).toBe("https://github.com/acme/demo/pull/1");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer");
    // the turn is still working (its own glyph spins), but no step of it is
    expect(turn.querySelectorAll("li svg.lucide-loader-circle")).toHaveLength(0);
    agent.stop();
  });

  it("the ask that failed says so on its own line, with why, closed; the turn counts it", async () => {
    const { agent } = await play(null);
    const turn = await pane();
    const asks = within(turn)
      .getAllByRole("button", { name: /^Asked / })
      .map((b) => b.textContent);
    // the first ask is the coder's (its researcher is inside it), the second the one that failed
    expect(asks).toEqual(["Asked Coder: Answered· 1 step", "Asked Researcher: Failed"]);
    const failed = askOf(turn, /^Asked Researcher/);
    expect(row(failed).getAttribute("data-ask-state")).toBe("failed");
    expect(row(failed).getAttribute("data-state")).toBe("failed");
    expect(failed.getAttribute("aria-expanded")).toBe("false");
    const note = row(failed).querySelector('[data-slot="ask-note"]') as HTMLElement;
    expect(note.textContent).toBe("Why: scripted failure");
    // the turn's header and the chat's line say one failed, and the first one is this ask
    expect(within(turn).getAllByText("1 failed").length).toBeGreaterThan(0);
    const chat = await screen.findByRole("button", {
      name: /^1 failed\. Show the first one in the side panel/,
    });
    expect(chat).toBeTruthy();
    agent.stop();
  });

  it("the chat's line follows the ask that runs: 'Asked Researcher · 3 steps'", async () => {
    const { agent } = await play(4);
    const line = await screen.findByRole("button", {
      name: /^Plain's steps: Asked Researcher · 3 steps/,
    });
    expect(line.getAttribute("aria-expanded")).toBe("false");
    agent.stop();
  });
});
