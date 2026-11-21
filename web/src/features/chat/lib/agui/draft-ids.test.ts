import { describe, expect, it } from "vitest";
import { DraftIds } from "./draft-ids";

describe("DraftIds", () => {
  // ADR 0082, C4: the id is the draft's — minted for the words in the box, kept while they are those
  // words, and minted again when the person edits them (the server binds an id to its text, 422
  // `message_id_reused`).
  it("gives one text one id, however many times it is asked", () => {
    const ids = new DraftIds();
    const first = ids.of("read the repo");
    expect(ids.of("read the repo")).toBe(first);
    expect(ids.of("read the repo")).toBe(first);
  });

  it("mints another id when the words change, and another again when they change back", () => {
    const ids = new DraftIds();
    const first = ids.of("read the repo");
    const edited = ids.of("read the repo, then open a PR");
    expect(edited).not.toBe(first);
    // the id belongs to the text as it is now, not to a history of the box
    expect(ids.of("read the repo")).not.toBe(first);
  });

  it("keeps the id of a text that was refused, and drops it once the message is the log's", () => {
    const ids = new DraftIds();
    const refused = ids.of("hi");
    // a refusal changes nothing: the words are back in the box, so the id is still theirs
    expect(ids.of("hi")).toBe(refused);
    ids.accepted("hi");
    // accepted: the words typed again are a new message, not a resend of the one the log holds
    expect(ids.of("hi")).not.toBe(refused);
  });

  it("only drops the binding of the text it accepted", () => {
    const ids = new DraftIds();
    const kept = ids.of("one");
    ids.accepted("another");
    expect(ids.of("one")).toBe(kept);
  });

  it("tells two drafts apart, so one person's box never reuses the id of another text", () => {
    const ids = new DraftIds();
    expect(ids.of("one")).not.toBe(ids.of("two"));
  });
});
