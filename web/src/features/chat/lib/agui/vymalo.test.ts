import { describe, expect, it } from "vitest";
import {
  parseAction,
  parseActor,
  parseAnswers,
  parseArtifact,
  parseCheck,
  parseCi,
  parseError,
  parseJob,
  parseRework,
  parseStatus,
  parseUiCatalog,
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
      // an artifact of an orchestrator that does not type them is a file
      kind: "file",
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

  it("typed artifacts (docs/api/agui.md, Typed artifacts): the kind and the fields of its card", () => {
    expect(
      parseArtifact({
        kind: "pull_request",
        name: "pull_request",
        url: "https://github.com/acme/demo/pull/12",
        number: 12,
        repository: "github.com/acme/demo",
        branch: "agent/fix",
        at: "2026-09-30T10:00:00Z",
      }),
    ).toEqual({
      kind: "pull_request",
      name: "pull_request",
      url: "https://github.com/acme/demo/pull/12",
      number: 12,
      repository: "github.com/acme/demo",
      branch: "agent/fix",
      at: "2026-09-30T10:00:00Z",
    });
    expect(
      parseArtifact({
        kind: "branch",
        name: "branch",
        repository: "github.com/acme/demo",
        branch: "agent/fix",
        sha: "0000000000000000000000000000000000000001",
        shortSha: "0000000",
      }),
    ).toMatchObject({ kind: "branch", branch: "agent/fix", shortSha: "0000000" });
    expect(parseArtifact({ kind: "checks", name: "checks", passed: false })).toMatchObject({
      kind: "checks",
      passed: false,
    });
    // a kind this UI does not know is a file; a URL that is not https is dropped
    expect(parseArtifact({ kind: "diagram", name: "d" })).toEqual({ kind: "file", name: "d" });
    expect(
      parseArtifact({ kind: "pull_request", name: "pr", url: "http://example.com/pull/1" }),
    ).toEqual({ kind: "pull_request", name: "pr" });
    expect(
      parseArtifact({ kind: "pull_request", name: "pr", url: "javascript:alert(1)", number: -1 }),
    ).toEqual({ kind: "pull_request", name: "pr" });
  });

  it("`at`, when it is a time, on every activity", () => {
    const at = "2026-09-30T10:00:00.123Z";
    expect(parseStatus({ status: "working", at })).toEqual({ status: "working", at });
    expect(parseStatus({ status: "working", at: "yesterday-ish" })).toEqual({ status: "working" });
    expect(parseError({ message: "down", at })).toMatchObject({ at });
    expect(parseArtifact({ name: "x", at })).toMatchObject({ at });
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

describe("reading a CI report (ADR 0017)", () => {
  const ci = {
    name: "ci/build",
    conclusion: "failure",
    passed: false,
    sha: "0000000000000000000000000000000000000001",
    shortSha: "0000000",
    provider: "generic",
    repository: "github.com/acme/demo",
    branch: "agent/fix",
    url: "https://ci.example.com/runs/1",
    summary: "1 test failed: tests::login",
    actor: { type: "system", name: "orchestrator" },
  };

  it("keeps what the schema names and ignores the rest", () => {
    expect(parseCi({ ...ci, futureField: { deep: [1] }, extra: "x" })).toEqual({
      name: "ci/build",
      conclusion: "failure",
      passed: false,
      sha: "0000000000000000000000000000000000000001",
      shortSha: "0000000",
      provider: "generic",
      repository: "github.com/acme/demo",
      branch: "agent/fix",
      url: "https://ci.example.com/runs/1",
      summary: "1 test failed: tests::login",
      actor: { type: "system", name: "orchestrator" },
    });
  });

  it("branch, url, summary and actor are optional", () => {
    const { branch, url, summary, actor, ...required } = ci;
    expect(parseCi(required)).toEqual(required);
  });

  it("a report that is not one renders nothing: every required member is needed", () => {
    for (const key of [
      "name",
      "conclusion",
      "passed",
      "sha",
      "shortSha",
      "provider",
      "repository",
    ] as const) {
      const { [key]: _gone, ...rest } = ci;
      expect(parseCi(rest), `without ${key}`).toBeNull();
    }
    expect(parseCi({ ...ci, name: "" })).toBeNull();
    expect(parseCi({ ...ci, name: 7 })).toBeNull();
    expect(parseCi({ ...ci, passed: "false" })).toBeNull();
    expect(parseCi({ ...ci, conclusion: null })).toBeNull();
    expect(parseCi({})).toBeNull();
    expect(parseCi(null)).toBeNull();
    expect(parseCi("ci/build")).toBeNull();
    expect(parseCi([ci])).toBeNull();
  });

  it("a conclusion it does not know is kept: `passed` says how it counts", () => {
    expect(parseCi({ ...ci, conclusion: "future_thing", passed: true })).toMatchObject({
      conclusion: "future_thing",
      passed: true,
    });
  });

  it("optional strings that are not strings are absent", () => {
    const read = parseCi({ ...ci, branch: 4, summary: { a: 1 }, url: ["https://x.example"] });
    expect(read).not.toHaveProperty("branch");
    expect(read).not.toHaveProperty("summary");
    expect(read).not.toHaveProperty("url");
  });

  it("a url that is not http(s) is dropped, whatever it looks like", () => {
    for (const url of [
      "javascript:alert(1)",
      "JaVaScRiPt:alert(1)",
      " javascript:alert(1)",
      "java\tscript:alert(1)",
      "data:text/html,<script>alert(1)</script>",
      "file:///etc/passwd",
      "blob:https://ci.example.com/x",
      "vbscript:x",
      "mailto:a@example.com",
      "//ci.example.com/runs/1",
      "/runs/1",
      "ci.example.com/runs/1",
      "https://user@evil.example/",
      "",
    ]) {
      expect(parseCi({ ...ci, url }), url).not.toHaveProperty("url");
      expect(parseCi({ ...ci, url })?.name, url).toBe("ci/build");
    }
    expect(parseCi({ ...ci, url: "http://ci.example.com/runs/1" })?.url).toBe(
      "http://ci.example.com/runs/1",
    );
    expect(parseCi({ ...ci, url: "HTTPS://ci.example.com/runs/1" })?.url).toBe(
      "https://ci.example.com/runs/1",
    );
  });
});

describe("thread.uiCatalog of a snapshot", () => {
  const digest = `sha256:${"a".repeat(64)}`;

  it("is read when it is a catalog this build can compare", () => {
    expect(
      parseUiCatalog({ catalogId: "https://x.test/c", version: 2, digest, extra: true }),
    ).toEqual({ catalogId: "https://x.test/c", version: 2, digest });
  });

  it("is nothing for anything else: it is not guessed", () => {
    for (const v of [
      undefined,
      null,
      "text",
      [],
      {},
      { catalogId: "https://x.test/c", version: 2 },
      { catalogId: "https://x.test/c", version: 0, digest },
      { catalogId: "https://x.test/c", version: 1.5, digest },
      { catalogId: "https://x.test/c", version: "2", digest },
      { catalogId: "", version: 2, digest },
      { catalogId: "https://x.test/c", version: 2, digest: "sha256:ABC" },
      { catalogId: "https://x.test/c", version: 2, digest: `SHA256:${"a".repeat(64)}` },
    ]) {
      expect(parseUiCatalog(v), JSON.stringify(v)).toBeNull();
    }
  });
});

describe("an action's context and the answer it may be", () => {
  const base = { surfaceId: "s1", name: "answer", sourceComponentId: "pick" };

  it("parseAction keeps the context when it is an object, and drops anything else", () => {
    expect(parseAction({ ...base, context: { choice: "a" } })).toEqual({
      ...base,
      context: { choice: "a" },
    });
    expect(parseAction({ ...base, context: [1] })).toEqual(base);
    expect(parseAction({ ...base, context: "x" })).toEqual(base);
    expect(parseAction(base)).toEqual(base);
  });

  it("parseAnswers is an action whose context has the answers of a Choices", () => {
    expect(
      parseAnswers({
        ...base,
        at: "2027-01-15T08:00:07Z",
        actor: { type: "user", name: "alice@example.com" },
        context: {
          answers: [
            { id: "db", values: ["pg"] },
            { id: "a", values: [], other: "x" },
          ],
        },
      }),
    ).toEqual({
      surfaceId: "s1",
      sourceComponentId: "pick",
      answers: [
        { id: "db", values: ["pg"] },
        { id: "a", values: [], other: "x" },
      ],
      actor: { type: "user", name: "alice@example.com" },
      at: "2027-01-15T08:00:07Z",
    });
  });

  it("parseAnswers is null for a button's action and for anything that is not an answer", () => {
    expect(parseAnswers({ ...base, context: { choice: "a" } })).toBeNull();
    expect(parseAnswers({ ...base, context: { answers: [{ id: 1, values: [] }] } })).toBeNull();
    expect(parseAnswers(base)).toBeNull();
    expect(parseAnswers("x")).toBeNull();
    expect(parseAnswers({ context: { answers: [{ id: "a", values: [] }] } })).toBeNull();
  });
});
