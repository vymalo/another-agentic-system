// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/*
 * Forking the open thread (ADR 0029) with the access tier (ADR 0061): an edit is sent under the
 * composer's pick, and the fork continues the conversation with the person's pick.
 */

const hoisted = vi.hoisted(() => ({
  post: vi.fn(),
  push: vi.fn(),
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: hoisted.push }) }));
vi.mock("@/lib/api/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/client")>()),
  api: { POST: hoisted.post },
}));

import { readPick, writePick } from "@/features/tier/lib/pick";
import { useFork } from "./use-fork";

const PARENT = "00000000-0000-4000-8000-000000000001";

beforeEach(() => {
  hoisted.post.mockReset();
  hoisted.push.mockReset();
  hoisted.post.mockImplementation(async (_path: string, init: { body: { id: string } }) => ({
    data: { id: init.body.id, forkedFrom: { threadId: PARENT, seq: 4, kind: "edit" } },
  }));
});
afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

const bodyOfCall = () => hoisted.post.mock.calls.at(-1)?.[1].body as Record<string, unknown>;

describe("useFork: the access tier of an edit", () => {
  it("is sent with the edit when the composer has a pick", async () => {
    const { result } = renderHook(() => useFork(PARENT));
    await act(() =>
      result.current.fork({ replace: 3, text: "say it again", messageId: "m-9", tier: "edits" }),
    );
    expect(bodyOfCall()).toMatchObject({ replace: 3, text: "say it again", tier: "edits" });
  });

  it("is left out when there is none: the orchestrator keeps the tier of the message it replaces", async () => {
    const { result } = renderHook(() => useFork(PARENT));
    await act(() => result.current.fork({ replace: 3, text: "say it again", messageId: "m-9" }));
    expect(bodyOfCall()).not.toHaveProperty("tier");
  });

  it("is never part of a fork from here, which has no message", async () => {
    const { result } = renderHook(() => useFork(PARENT));
    await act(() => result.current.fork({ after: 4 }));
    expect(bodyOfCall()).not.toHaveProperty("tier");
  });
});

describe("useFork: the pick of the person goes with the fork", () => {
  it("the new thread has the pick of the thread it continues", async () => {
    writePick(PARENT, "bypass");
    const { result } = renderHook(() => useFork(PARENT));
    await act(() => result.current.fork({ after: 4 }));
    const made = hoisted.push.mock.calls[0]?.[0] as string;
    const id = made.replace("/threads/", "");
    expect(readPick(id)).toBe("bypass");
  });

  it("a person who picked nothing leaves the fork with nothing", async () => {
    const { result } = renderHook(() => useFork(PARENT));
    await act(() => result.current.fork({ replace: 3, text: "again", messageId: "m-1" }));
    const made = String(hoisted.push.mock.calls[0]?.[0]).split("#")[0] ?? "";
    expect(readPick(made.replace("/threads/", ""))).toBeUndefined();
  });

  it("a fork that was refused changes nothing", async () => {
    writePick(PARENT, "plan");
    hoisted.post.mockResolvedValueOnce({ error: { status: 422, detail: "no" } });
    const { result } = renderHook(() => useFork(PARENT));
    await act(() => result.current.fork({ after: 4 }));
    expect(hoisted.push).not.toHaveBeenCalled();
    expect(window.localStorage.length).toBe(1);
  });
});
