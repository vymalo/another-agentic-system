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
const action = (id: string, surfaceId: string, role = "user") => ({
  id,
  role,
  content: [data(ACTIVITY.action, { surfaceId, name: "answer", context: { answers: [] } })],
});
const words = (id: string) => ({
  id,
  role: "user",
  content: [{ type: "text", text: "use Postgres" }],
});
const agent = (id: string) => ({ id, role: "assistant", content: [{ type: "text", text: "ok" }] });

describe("answeredAfter: a form the person has acted after is answered, even when the thread waits again", () => {
  it("is false for a surface nobody has acted after", () => {
    expect(answeredAfter([surface("a2ui-1", "s1")], "a2ui-1")).toBe(false);
    expect(answeredAfter([surface("a2ui-1", "s1"), agent("a-2")], "a2ui-1")).toBe(false);
  });

  it("is true once an action comes after it, whichever way the transcript tags it", () => {
    expect(answeredAfter([surface("a2ui-1", "s1"), action("evt-2", "s1")], "a2ui-1")).toBe(true);
    expect(
      answeredAfter([surface("a2ui-1", "s1"), action("evt-2", "s1", "assistant")], "a2ui-1"),
    ).toBe(true);
  });

  it("is true once a plain message of the person comes after it: it answered the ask in words", () => {
    expect(answeredAfter([surface("a2ui-1", "s1"), words("evt-2")], "a2ui-1")).toBe(true);
  });

  it("an action on a later form closes the earlier one too", () => {
    const messages = [surface("a2ui-1", "s1"), surface("a2ui-2", "s2"), action("evt-3", "s2")];
    expect(answeredAfter(messages, "a2ui-1")).toBe(true);
    expect(answeredAfter(messages, "a2ui-2")).toBe(true);
  });

  it("is about what came after: input before the form is not an answer to it", () => {
    const messages = [action("evt-1", "s1"), words("evt-2"), surface("a2ui-3", "s1")];
    expect(answeredAfter(messages, "a2ui-3")).toBe(false);
  });

  it("a form that asks again is open while the first stays answered", () => {
    const messages = [surface("a2ui-1", "s1"), action("evt-2", "s1"), surface("a2ui-3", "s2")];
    expect(answeredAfter(messages, "a2ui-1")).toBe(true);
    expect(answeredAfter(messages, "a2ui-3")).toBe(false);
  });

  it("is false for a message the transcript does not hold", () => {
    expect(answeredAfter([action("evt-2", "s1")], "a2ui-9")).toBe(false);
  });
});
