import { describe, expect, it } from "vitest";
import {
  parseAction,
  parseActor,
  parseArtifact,
  parseCheck,
  parseError,
  parseJob,
  parseRework,
  parseStatus,
} from "./vymalo";

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

describe("reading the verification gate's activities (ADR 0018)", () => {
  const check = {
    source: "agent_checks",
    attempt: 1,
    status: "failed",
    commit: "0000000000000000000000000000000000000001",
    summary: "1 test failed",
    findings: ["tests::login fails: expected 200, got 500"],
    actor: { type: "system", name: "orchestrator" },
  };

  it("a check keeps what the schema names and ignores the rest", () => {
    expect(parseCheck({ ...check, futureField: { deep: true }, name: "build" })).toEqual({
      source: "agent_checks",
      attempt: 1,
      status: "failed",
      name: "build",
      commit: "0000000000000000000000000000000000000001",
      summary: "1 test failed",
      stale: false,
      findings: ["tests::login fails: expected 200, got 500"],
      actor: { type: "system", name: "orchestrator" },
    });
    expect(parseCheck({ source: "ci", attempt: 2, status: "pending" })).toEqual({
      source: "ci",
      attempt: 2,
      status: "pending",
      stale: false,
      findings: [],
    });
    expect(parseCheck({ source: "ci", attempt: 2, status: "passed", stale: true })).toMatchObject({
      stale: true,
    });
    // `stale` is only ever `true`
    expect(parseCheck({ source: "ci", attempt: 2, status: "passed", stale: "yes" })).toMatchObject({
      stale: false,
    });
  });

  it("a check that is not one renders nothing: no source, attempt or known status", () => {
    for (const bad of [
      null,
      "failed",
      [],
      {},
      { ...check, source: undefined },
      { ...check, source: "" },
      { ...check, source: 7 },
      { ...check, attempt: 0 },
      { ...check, attempt: 1.5 },
      { ...check, attempt: "1" },
      { ...check, attempt: undefined },
      { ...check, status: "exploding" },
      { ...check, status: undefined },
    ]) {
      expect(parseCheck(bad)).toBeNull();
    }
  });

  it("findings that are not text are dropped, not guessed; a list that is not a list is empty", () => {
    expect(
      parseCheck({ ...check, findings: ["a", 1, null, { x: 1 }, ["b"], "c"] })?.findings,
    ).toEqual(["a", "c"]);
    expect(parseCheck({ ...check, findings: "one big string" })?.findings).toEqual([]);
    expect(parseCheck({ ...check, findings: { 0: "a" } })?.findings).toEqual([]);
    // a hostile list is read only so far
    const many = Array.from({ length: 5000 }, (_, i) => `finding ${i}`);
    expect(parseCheck({ ...check, findings: many })?.findings).toHaveLength(100);
  });

  it("strings that are not strings are absent", () => {
    expect(parseCheck({ ...check, summary: 4, commit: {}, name: [] })).toMatchObject({
      status: "failed",
    });
    const parsed = parseCheck({ ...check, summary: 4, commit: {}, name: [] });
    expect(parsed).not.toHaveProperty("summary");
    expect(parsed).not.toHaveProperty("commit");
    expect(parsed).not.toHaveProperty("name");
  });

  it("a rework says which attempt starts, of how many, and what was sent back", () => {
    expect(
      parseRework({
        attempt: 2,
        maxAttempts: 3,
        findings: [
          { source: "agent_checks", findings: ["a", "b", 3], extra: 1 },
          { source: "ci", findings: [] },
          { findings: ["no source"] },
          "junk",
        ],
        futureField: 1,
      }),
    ).toEqual({
      attempt: 2,
      maxAttempts: 3,
      findings: [
        { source: "agent_checks", findings: ["a", "b"] },
        { source: "ci", findings: [] },
      ],
    });
    expect(parseRework({ attempt: 3, maxAttempts: 3 })).toEqual({
      attempt: 3,
      maxAttempts: 3,
      findings: [],
    });
    expect(parseRework({ attempt: 3, maxAttempts: 3, findings: "none" })?.findings).toEqual([]);
  });

  it("a rework without its attempts renders nothing", () => {
    for (const bad of [
      null,
      [],
      {},
      { attempt: 2 },
      { maxAttempts: 3 },
      { attempt: 0, maxAttempts: 3 },
    ]) {
      expect(parseRework(bad)).toBeNull();
    }
  });

  it("the job of a snapshot: attempt, attempts there are, sources, and the commit once pushed", () => {
    expect(
      parseJob({ attempt: 2, maxAttempts: 3, gate: ["agent_checks", 4], sha: "abc", extra: 1 }),
    ).toEqual({ attempt: 2, maxAttempts: 3, gate: ["agent_checks"], sha: "abc" });
    expect(parseJob({ attempt: 1, maxAttempts: 3 })).toEqual({
      attempt: 1,
      maxAttempts: 3,
      gate: [],
    });
    // no gate, no job: absent, or not a job
    for (const bad of [
      undefined,
      null,
      "2/3",
      [],
      {},
      { attempt: 1 },
      { attempt: 0, maxAttempts: 3 },
    ]) {
      expect(parseJob(bad)).toBeNull();
    }
  });
});
