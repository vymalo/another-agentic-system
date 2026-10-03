import { describe, expect, it, vi } from "vitest";
import type { ApiSharedThread } from "@/lib/api/types";
import { type Reply, resolveShare } from "./resolve";

const TOKEN = "T".repeat(43);
const THREAD: ApiSharedThread = {
  id: "11111111-1111-4111-8111-111111111111",
  title: "A shared one",
  target: { agentId: "coder" },
  state: "done",
  lastSeq: 7,
  visibility: "internal",
  isOwner: false,
  createdAt: "2026-10-03T09:00:00Z",
  updatedAt: "2026-10-03T09:05:00Z",
};

/** The two routes, each answering what the test says; a route that is not named was not to be asked. */
function routes(answers: { signedIn?: Reply; public?: Reply }) {
  const asked: string[] = [];
  const get = vi.fn(async (route: "signed-in" | "public"): Promise<Reply> => {
    asked.push(route);
    const answer = route === "public" ? answers.public : answers.signedIn;
    if (!answer) throw new Error(`${route} was not to be asked`);
    return answer;
  });
  return { get, asked };
}

describe("resolveShare: the link of a shared thread", () => {
  it("is read by the signed-in route first, as an internal reader", async () => {
    const { get, asked } = routes({ signedIn: { status: 200, data: THREAD } });
    const signIn = vi.fn(() => true);
    expect(await resolveShare(TOKEN, get, signIn)).toEqual({
      status: "ready",
      thread: THREAD,
      source: { token: TOKEN, audience: "internal" },
    });
    expect(asked).toEqual(["signed-in"]);
    expect(signIn).not.toHaveBeenCalled();
  });

  it("sends the owner to the thread itself", async () => {
    const { get } = routes({ signedIn: { status: 200, data: { ...THREAD, isOwner: true } } });
    expect(await resolveShare(TOKEN, get, () => true)).toEqual({
      status: "owner",
      threadId: THREAD.id,
    });
  });

  it("on a 401 (not signed in) asks the public route, and reads as anybody", async () => {
    const { get, asked } = routes({
      signedIn: { status: 401 },
      public: { status: 200, data: { ...THREAD, visibility: "public" } },
    });
    const signIn = vi.fn(() => true);
    const view = await resolveShare(TOKEN, get, signIn);
    expect(view).toMatchObject({ status: "ready", source: { token: TOKEN, audience: "public" } });
    expect(asked).toEqual(["signed-in", "public"]);
    expect(signIn).not.toHaveBeenCalled();
  });

  it("when the public route is a 404 too, sends the browser to sign in, and waits for it", async () => {
    const { get } = routes({ signedIn: { status: 401 }, public: { status: 404 } });
    const signIn = vi.fn(() => true);
    expect(await resolveShare(TOKEN, get, signIn)).toEqual({ status: "signing-in" });
    expect(signIn).toHaveBeenCalledTimes(1);
  });

  it("with no sign-in to go to, or one that was just tried, says the link does not work", async () => {
    const { get } = routes({ signedIn: { status: 401 }, public: { status: 404 } });
    expect(await resolveShare(TOKEN, get, () => false)).toEqual({ status: "gone" });
  });

  it("says the same for every refusal of a signed-in reader, and never asks the public route", async () => {
    for (const status of [404, 403]) {
      const { get, asked } = routes({ signedIn: { status } });
      expect(await resolveShare(TOKEN, get, () => true)).toEqual({ status: "gone" });
      expect(asked).toEqual(["signed-in"]);
    }
  });

  it("is told to wait when the link is read too often, on either route", async () => {
    const limited = routes({ signedIn: { status: 401 }, public: { status: 429 } });
    expect(await resolveShare(TOKEN, limited.get, () => true)).toMatchObject({ status: "error" });
    const first = routes({ signedIn: { status: 429 } });
    expect(await resolveShare(TOKEN, first.get, () => true)).toMatchObject({ status: "error" });
  });

  it("is an error to retry, in the server's words, for anything else", async () => {
    const { get } = routes({ signedIn: { status: 500, detail: "the store is down" } });
    expect(await resolveShare(TOKEN, get, () => true)).toEqual({
      status: "error",
      message: "the store is down",
    });
  });

  it("is an error when the network fails", async () => {
    const get = vi.fn(async () => {
      throw new TypeError("network down");
    });
    expect(await resolveShare(TOKEN, get, () => true)).toEqual({
      status: "error",
      message: "network down",
    });
  });

  it("is gone for a token that cannot be one, without asking", async () => {
    const get = vi.fn();
    expect(await resolveShare("short", get, () => true)).toEqual({ status: "gone" });
    expect(get).not.toHaveBeenCalled();
  });
});
