import { AssistantRuntimeProvider } from "@assistant-ui/react";
import { type AgUiAssistantRuntime, useAgUiRuntime } from "@assistant-ui/react-ag-ui";
import { render } from "@testing-library/react";
import { LiveRuns } from "@/features/chat/components/live-runs";
import { type Call, fakeFetch, THREAD_ID } from "./testing";
import { ThreadAgent, type ThreadAgentOptions } from "./thread-agent";

/**
 * Test support: the real runtime (`@assistant-ui/react-ag-ui`, patched) over a ThreadAgent that
 * talks to a fake orchestrator, with the live-run driver mounted. jsdom only.
 */
export function mountRuntime(
  handler: (call: Call) => Response | Promise<Response>,
  options: Partial<ThreadAgentOptions> = {},
) {
  const { fetch, calls } = fakeFetch(handler);
  const agent = new ThreadAgent({
    threadId: THREAD_ID,
    fetch,
    baseUrl: "http://orch.test",
    target: () => ({ agentId: "plain", release: null }),
    backoff: () => 1,
    ...options,
  });
  const box: { runtime?: AgUiAssistantRuntime; errors: Error[] } = { errors: [] };
  function Harness() {
    const runtime = useAgUiRuntime({
      agent,
      resumeTranscript: "appended",
      onError: (e) => void box.errors.push(e),
    });
    box.runtime = runtime;
    return (
      <AssistantRuntimeProvider runtime={runtime}>
        <LiveRuns agent={agent} runtime={runtime} />
      </AssistantRuntimeProvider>
    );
  }
  const view = render(<Harness />);
  const runtime = () => box.runtime as AgUiAssistantRuntime;
  return { agent, calls, box, runtime, messages: () => runtime().thread.getState().messages, view };
}

type Message = ReturnType<AgUiAssistantRuntime["thread"]["getState"]>["messages"][number];

/** One line per part: `status:working`, `artifact`, `text:...`, `actor`. */
export type Summary = { role: string; status?: string; parts: string[] }[];

export function summarize(messages: readonly Message[]): Summary {
  return messages.map((m) => ({
    role: m.role,
    ...(m.role === "assistant" && m.status
      ? {
          status:
            m.status.type === "complete"
              ? "complete"
              : `${m.status.type}:${"reason" in m.status ? m.status.reason : ""}`,
        }
      : {}),
    parts: m.content.map((p) => {
      if (p.type === "text") return `text:${p.text}`;
      if (p.type !== "data") return p.type;
      const name = p.name.replace("agui-activity/vymalo.", "").replace("vymalo.", "");
      const data = p.data as { status?: string } | undefined;
      return data?.status ? `${name}:${data.status}` : name;
    }),
  }));
}
