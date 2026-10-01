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
import { LiveStream, loadGolden, sse } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import { StepsPanelTestProvider } from "@/features/panel/hooks/use-steps-panel";
import { ThreadViewProvider } from "../thread-view";
import { StepsPanelContent } from "./steps-panel-content";
import { TurnSummaryLine } from "./turn-summary";

configure({ asyncUtilTimeout: 10_000 });

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
});
afterEach(cleanup);

/**
 * The connected forms, over the real runtime playing a golden: what the shell renders in its
 * Activity tab and what the chat renders under a turn's header.
 */
async function play(golden: string, view = { state: "done" as const, waiting: false }) {
  const stream = new LiveStream();
  const mounted = mountRuntime(
    () => sse(stream.body),
    {},
    <StepsPanelTestProvider>
      <ThreadViewProvider value={{ ...view, agentId: "plain" }}>
        <StepsPanelContent />
        <Summaries />
      </ThreadViewProvider>
    </StepsPanelTestProvider>,
  );
  mounted.agent.start();
  const frames = loadGolden(golden);
  await act(async () => {
    stream.frames(frames);
  });
  await waitFor(() => expect(mounted.agent.getSnapshot().lastSeq).toBe(frames.at(-1)?.id));
  await waitFor(() => expect(mounted.runtime().thread.getState().isRunning).toBe(false));
  return mounted;
}

/** One summary line per assistant message, as the transcript draws them. */
function Summaries() {
  const ids = useTurnIds();
  return (
    <div data-testid="chat">
      {ids.map((id) => (
        <TurnSummaryLine key={id} turnId={id} />
      ))}
    </div>
  );
}

function useTurnIds(): string[] {
  const joined = useAuiState((s) =>
    s.thread.messages
      .filter((m) => m.role === "assistant")
      .map((m) => m.id)
      .join(","),
  );
  return joined ? joined.split(",") : [];
}

describe("the connected tree over the runtime", () => {
  it("draws the steps golden: the sub-agent and its failing command, in the pane", async () => {
    const { agent } = await play("steps");
    const pane = await screen.findByRole("region", { name: /^Turn 1/ });
    expect(within(pane).getByText("Started working")).toBeTruthy();
    const opencode = within(pane).getByRole("button", { name: /^OpenCode/ });
    expect(opencode.textContent).toBe("OpenCode· 1 step");
    fireEvent.click(opencode);
    expect(within(pane).getByText("Command failed")).toBeTruthy();
    expect(within(pane).getAllByText("1 failed").length).toBeGreaterThan(0);
    agent.stop();
  });

  it("gives the chat one line for the turn, which asks the panel to show it", async () => {
    const { agent } = await play("steps");
    const line = await screen.findByRole("button", {
      name: /^Plain's steps: 3 steps · 6s, 1 failed/,
    });
    expect(line.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(line);
    // the test provider is a panel that opened: the pane was asked to show the turn
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /^Plain's steps/ }).getAttribute("aria-expanded"),
      ).toBe("true"),
    );
    agent.stop();
  });

  it("lists both turns of a thread that asked a question, the first paused", async () => {
    const { agent } = await play("steps-ask");
    // the second run is applied once the first turn has settled in the transcript
    await waitFor(() => expect(screen.getAllByRole("heading", { level: 3 })).toHaveLength(2));
    const headings = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(headings).toEqual(["Turn 1 · Plain · 3s", "Turn 2 · Plain · 3s"]);
    expect(screen.getByRole("button", { name: /^Plain's steps: Paused · 3 steps/ })).toBeTruthy();
    agent.stop();
  });
});
