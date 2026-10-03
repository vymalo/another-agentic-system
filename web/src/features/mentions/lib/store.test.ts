import { describe, expect, it } from "vitest";
import { labelOf, type Mention } from "./mentions";
import { MentionsStore } from "./store";

const at = (text: string, id: string): Mention => {
  const label = labelOf(id);
  const start = text.indexOf(label);
  return { agentId: id, label, start, end: start + label.length };
};

/** A store whose box holds `text` with a mention of each id, as the picks would have left it. */
function holding(text: string, ...ids: string[]) {
  const store = new MentionsStore();
  store.set(
    text,
    ids.map((id) => at(text, id)),
  );
  return store;
}

describe("the mentions of the box", () => {
  it("follow the text as it is edited, and go when it is emptied", () => {
    const store = holding("ask @coder", "coder");
    store.sync("please ask @coder");
    expect(store.current).toEqual([at("please ask @coder", "coder")]);
    store.sync("please ask @code");
    expect(store.current).toEqual([]);
  });

  it("take gives a message its mentions, whether or not the box was already emptied", () => {
    const store = holding("😄 @coder", "coder");
    expect(store.take("😄 @coder")).toEqual([at("😄 @coder", "coder")]);
    store.sync(""); // the runtime cleared the box before the run was posted
    expect(store.take("😄 @coder")).toEqual([at("😄 @coder", "coder")]);
  });

  it("take moves them to the trimmed text the runtime may send, and gives none for another text", () => {
    const store = holding("  @coder  ", "coder");
    expect(store.take("@coder")).toEqual([at("@coder", "coder")]);
    expect(store.take("something else")).toEqual([]);
  });

  it("a message's mentions are kept by its id for the bubble, and forgotten when it is refused", () => {
    const store = holding("@coder", "coder");
    let changes = 0;
    store.subscribe(() => changes++);
    store.take("@coder", "m-1");
    expect(store.sentOf("m-1")).toEqual([at("@coder", "coder")]);
    expect(changes).toBe(1);
    store.refused("m-1");
    expect(store.sentOf("m-1")).toBeUndefined();
    store.take("nothing here", "m-2");
    expect(store.sentOf("m-2")).toBeUndefined();
  });

  it("the text that comes back after a refused send brings its mentions back", () => {
    const store = holding("ask @coder", "coder");
    store.take("ask @coder", "m-1");
    store.sync(""); // sent: the box is empty
    expect(store.current).toEqual([]);
    store.sync("ask @coder"); // refused: the runtime put the text back
    expect(store.current).toEqual([at("ask @coder", "coder")]);
  });

  it("other text that arrives is not given the mentions of what was sent", () => {
    const store = holding("ask @coder", "coder");
    store.sync("");
    store.sync("ask @coder please"); // typed anew: no pick, no mention
    expect(store.current).toEqual([]);
  });

  it("once the run is accepted what the box held is the log's: the same text typed again has none", () => {
    const store = holding("ask @coder", "coder");
    store.sync("");
    store.accepted();
    store.sync("ask @coder");
    expect(store.current).toEqual([]);
  });

  it("restoreInFront puts a refused message's words and mentions in front of what was written since", () => {
    const store = holding("@coder first", "coder");
    const sent = store.take("@coder first");
    store.sync("");
    store.set("then @verifier", [at("then @verifier", "verifier")]);
    const text = store.restoreInFront("@coder first", sent, "then @verifier");
    expect(text).toBe("@coder first\n\nthen @verifier");
    expect(store.current.map((m) => m.agentId)).toEqual(["coder", "verifier"]);
    for (const m of store.current) expect(text.slice(m.start, m.end)).toBe(m.label);
    // with nothing written since, the text is as it was
    const alone = holding("@coder", "coder");
    expect(alone.restoreInFront("@coder", alone.take("@coder"), "  ")).toBe("@coder");
  });

  it("followList moves a card URL to what the list says now", () => {
    const store = holding("@coder", "coder");
    store.set("@coder", [{ ...at("@coder", "coder"), cardUrl: "http://old" }]);
    store.followList([{ id: "coder", name: "Coder", cardUrl: "http://new" }]);
    expect(store.current[0]?.cardUrl).toBe("http://new");
    store.followList([{ id: "coder", name: "Coder" }]); // no card URL listed: left as it is
    expect(store.current[0]?.cardUrl).toBe("http://new");
  });
});
