// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { navigation, resetRedirectPause } from "@/lib/api/session";
import { fakeFetch, problem, THREAD_ID } from "./testing";
import { ThreadAgent } from "./thread-agent";

/*
 * The connect stream is the first request to meet an expired session (it reconnects by itself): a
 * 401 on it, or on a send, goes to the edge's sign-in when the deployment has one, and is the same
 * failure as before when it has none.
 */

let go: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  resetRedirectPause();
  window.history.pushState({}, "", `/threads/${THREAD_ID}`);
  go = vi.spyOn(navigation, "go").mockImplementation(() => {});
});
afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

const agentWith = () => {
  const { fetch } = fakeFetch(() => problem(401, "Unauthorized", "the session has expired"));
  return new ThreadAgent({
    threadId: THREAD_ID,
    fetch,
    baseUrl: "http://orch.test",
    target: () => ({ agentId: "plain", release: null }),
    backoff: () => 1_000_000,
  });
};
const until = async (test: () => boolean) => {
  for (let i = 0; i < 400 && !test(); i++) await new Promise((r) => setTimeout(r, 5));
  expect(test()).toBe(true);
};

describe("ThreadAgent and an expired session", () => {
  it("a 401 on the connect stream goes to the sign-in with the thread's page as rd", async () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/sign_in");
    const agent = agentWith();
    agent.start();
    await until(() => go.mock.calls.length > 0);
    expect(go).toHaveBeenCalledWith(
      `/oauth2/sign_in?rd=${encodeURIComponent(`/threads/${THREAD_ID}`)}`,
    );
    agent.stop();
  });

  it("without a sign-in path it is reported as it was, and nothing navigates", async () => {
    const agent = agentWith();
    agent.start();
    await until(() => agent.getSnapshot().error !== null);
    expect(agent.getSnapshot().error).toBe("the session has expired");
    expect(go).not.toHaveBeenCalled();
    agent.stop();
  });
});
