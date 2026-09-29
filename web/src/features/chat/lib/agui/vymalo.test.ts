import { describe, expect, it } from "vitest";
import { parseAction, parseActor, parseArtifact, parseError, parseStatus } from "./vymalo";

describe("reading an activity's content", () => {
  it("keeps what is known and drops what is not", () => {
    expect(
      parseStatus({
        status: "input_required",
        detail: "Which branch?",
        actor: { type: "agent", name: "coder", revision: "coder-r47", extra: 1 },
        other: true,
      }),
    ).toEqual({
      status: "input_required",
      detail: "Which branch?",
      actor: { type: "agent", name: "coder", revision: "coder-r47" },
    });
    expect(parseStatus({ status: "working" })).toEqual({ status: "working" });
  });

  it("refuses a shape it does not know: nothing is drawn, nothing is guessed", () => {
    expect(parseStatus({ status: "exploding" })).toBeNull();
    expect(parseStatus("working")).toBeNull();
    expect(parseStatus(null)).toBeNull();
    expect(parseArtifact({ text: "no name" })).toBeNull();
    expect(parseArtifact([])).toBeNull();
    expect(parseError({ retryable: true })).toBeNull();
    expect(parseAction({ surfaceId: "s" })).toBeNull();
    expect(parseActor({ type: "robot", name: "x" })).toBeUndefined();
    expect(parseActor({ type: "user" })).toBeUndefined();
  });

  it("artifacts and errors", () => {
    expect(
      parseArtifact({
        name: "result",
        uri: "https://github.com/acme/demo/pull/1",
        text: "x",
        mimeType: "text/plain",
      }),
    ).toEqual({
      name: "result",
      uri: "https://github.com/acme/demo/pull/1",
      text: "x",
      mimeType: "text/plain",
    });
    expect(parseError({ message: "down", retryable: true })).toEqual({
      message: "down",
      retryable: true,
    });
    expect(parseError({ message: "down" })).toEqual({ message: "down", retryable: false });
  });
});
