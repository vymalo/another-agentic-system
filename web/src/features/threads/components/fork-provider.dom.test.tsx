// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import { ForkProvider, type ThreadFork, useThreadFork } from "./fork-provider";

/*
 * Regenerate is an edit with the same words (ADR 0029): the request has a message id of its own, kept
 * for a retry after a refusal and never after a success, and it names no access tier even when the
 * composer has one (the server keeps the tier of the message it replaces).
 */

const fork = vi.fn();
vi.mock("@/features/threads/hooks/use-fork", () => ({
  useFork: () => ({ busy: false, error: null, clearError: () => {}, fork }),
}));

const agent = { seqOfUser: () => undefined, endOfRun: () => undefined } as unknown as ThreadAgent;

let api: ThreadFork;
function Probe() {
  api = useThreadFork();
  return null;
}
const mount = (threadId = "t-1") =>
  render(
    <ForkProvider threadId={threadId} agent={agent} state="done" lastSeq={9} tier="edits">
      <Probe />
    </ForkProvider>,
  );
const bodies = () => fork.mock.calls.map((c) => c[0] as Record<string, unknown>);

afterEach(() => {
  cleanup();
  fork.mockReset();
});

describe("ForkProvider.regenerate", () => {
  it("sends the message's words and a message id, and no tier even when the composer has one", async () => {
    fork.mockResolvedValue(true);
    mount();
    await act(() => api.regenerate(3, "echo first"));
    expect(bodies()).toHaveLength(1);
    expect(bodies()[0]).toMatchObject({ replace: 3, text: "echo first" });
    expect(typeof bodies()[0]?.messageId).toBe("string");
    expect(bodies()[0]).not.toHaveProperty("tier");
  });

  it("keeps the message id for a retry after a refusal", async () => {
    fork.mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    mount();
    await act(() => api.regenerate(3, "echo first"));
    await act(() => api.regenerate(3, "echo first"));
    expect(bodies()[1]?.messageId).toBe(bodies()[0]?.messageId);
  });

  it("makes a new message id after a success, for the same seq and words", async () => {
    fork.mockResolvedValue(true);
    mount();
    await act(() => api.regenerate(3, "echo first"));
    await act(() => api.regenerate(3, "echo first"));
    expect(bodies()[1]?.messageId).not.toBe(bodies()[0]?.messageId);
  });

  it("does not share an id between two threads with the same seq and words", async () => {
    fork.mockResolvedValue(false);
    const a = mount("t-1");
    await act(() => api.regenerate(3, "echo first"));
    a.unmount();
    mount("t-2");
    await act(() => api.regenerate(3, "echo first"));
    expect(bodies()[1]?.messageId).not.toBe(bodies()[0]?.messageId);
  });

  it("says what kind of request was refused", async () => {
    fork.mockResolvedValue(false);
    mount();
    await act(() => api.regenerate(3, "echo first"));
    expect(api.errorKind).toBe("regenerate");
  });
});
