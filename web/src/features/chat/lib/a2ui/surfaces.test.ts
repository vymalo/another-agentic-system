import { describe, expect, it } from "vitest";
import { ACTIVITY, activityPartName } from "@/features/chat/lib/agui/vymalo";
import { answeredAfter } from "./surfaces";

const data = (type: string, payload: Record<string, unknown>) => ({
  type: "data",
  name: activityPartName(type),
  data: payload,
});
const surface = (id: string, surfaceId: string) => ({
  id,
  role: "assistant",
  content: [data(ACTIVITY.surface, { surface: surfaceId, a2ui_operations: [] })],
});
const action = (id: string, surfaceId: string) => ({
  id,
  role: "user",
  content: [data(ACTIVITY.action, { surfaceId, name: "answer", context: { answers: [] } })],
});

describe("answeredAfter: a form that was sent is answered, even when the thread waits again", () => {
  it("is false for a surface nobody has acted on", () => {
    expect(answeredAfter([surface("a2ui-1", "s1")], "a2ui-1", "s1")).toBe(false);
  });

  it("is true once an action names the surface after it", () => {
    const messages = [surface("a2ui-1", "s1"), action("evt-2", "s1")];
    expect(answeredAfter(messages, "a2ui-1", "s1")).toBe(true);
  });

  it("is about this surface: an action on another surface answers nothing here", () => {
    const messages = [surface("a2ui-1", "s1"), action("evt-2", "s2"), surface("a2ui-3", "s2")];
    expect(answeredAfter(messages, "a2ui-1", "s1")).toBe(false);
    expect(answeredAfter(messages, "a2ui-3", "s2")).toBe(false);
  });

  it("is about what came after: an action before the surface is not an answer to it", () => {
    const messages = [action("evt-1", "s1"), surface("a2ui-2", "s1")];
    expect(answeredAfter(messages, "a2ui-2", "s1")).toBe(false);
  });

  it("a second form that asks again is open while the first stays answered", () => {
    const messages = [surface("a2ui-1", "s1"), action("evt-2", "s1"), surface("a2ui-3", "s2")];
    expect(answeredAfter(messages, "a2ui-1", "s1")).toBe(true);
    expect(answeredAfter(messages, "a2ui-3", "s2")).toBe(false);
  });

  it("is false for a message the transcript does not hold", () => {
    expect(answeredAfter([action("evt-2", "s1")], "a2ui-9", "s1")).toBe(false);
  });
});
