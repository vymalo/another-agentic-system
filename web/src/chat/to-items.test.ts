import { describe, expect, it } from "vitest";
import { applyEvents, emptyLog, visibleEvents } from "./event-log";
import {
  agentMessage,
  artifact,
  errorEvent,
  status,
  THREAD_ID,
  threadState,
  userMessage,
} from "./fixtures";
import { convertMessage, toItems } from "./to-items";

const build = () =>
  toItems(
    visibleEvents(
      applyEvents(emptyLog(THREAD_ID), [
        userMessage(1, "Do it"),
        status(2, "working"),
        agentMessage(3, "m1", "Working on **it**", false),
        agentMessage(4, "m1", "Done", true),
        artifact(5, "https://github.com/vymalo/example/pull/1"),
        errorEvent(6, "Agent crashed", true),
        status(7, "input_required", "Which branch?"),
        threadState(8, "blocked"),
      ]),
    ),
  );

describe("toItems / convertMessage", () => {
  it("maps each visible event to one item and hides thread_state", () => {
    const items = build();
    expect(items.map((i) => i.kind)).toEqual([
      "user",
      "status",
      "agent_text",
      "artifact",
      "error",
      "status",
    ]);
  });

  it("gives agent text a stable id across partial to final", () => {
    const item = build().find((i) => i.kind === "agent_text");
    expect(item?.id).toBe("msg-m1");
    expect(item && item.kind === "agent_text" && item.text).toBe("Done");
  });

  it("converts user and agent text with the actor in metadata", () => {
    const [user, , agent] = build().map((i) => convertMessage(i, 0));
    expect(user).toMatchObject({
      role: "user",
      content: [{ type: "text", text: "Do it" }],
      metadata: { custom: { actor: { type: "user", name: "dev@example.com" } } },
    });
    expect(agent).toMatchObject({
      role: "assistant",
      status: { type: "complete" },
      metadata: { custom: { actor: { name: "coder", revision: "coder-r47" } } },
    });
  });

  it("marks a partial agent message as running", () => {
    const items = toItems(
      visibleEvents(applyEvents(emptyLog(THREAD_ID), [agentMessage(1, "m", "Hel", false)])),
    );
    expect(items[0] && convertMessage(items[0], 0).status).toEqual({ type: "running" });
  });

  it("converts status, artifact (with PR detection) and error to data parts", () => {
    const converted = build().map((i) => convertMessage(i, 0));
    expect(converted[1]?.content).toEqual([
      { type: "data-status", data: expect.objectContaining({ status: "working" }) },
    ]);
    expect(converted[3]?.content).toEqual([
      {
        type: "data-artifact",
        data: expect.objectContaining({
          name: "pull-request",
          pr: {
            label: "vymalo/example#1",
            href: "https://github.com/vymalo/example/pull/1",
          },
        }),
      },
    ]);
    expect(converted[4]?.content).toEqual([
      {
        type: "data-error",
        data: expect.objectContaining({ message: "Agent crashed", retryable: true }),
      },
    ]);
    expect(converted[5]?.content).toEqual([
      {
        type: "data-status",
        data: expect.objectContaining({ status: "input_required", detail: "Which branch?" }),
      },
    ]);
  });
});
