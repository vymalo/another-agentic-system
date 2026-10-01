import { describe, expect, it } from "vitest";
import { ACTIVITY, ACTOR_PART, activityPartName } from "./agui/vymalo";
import {
  checkLabel,
  checksPayload,
  commandOf,
  drawsStep,
  isStepPart,
  pullRequestOf,
  reworkLabel,
  shortRepository,
} from "./steps";

const data = (activity: string, payload: unknown) => ({
  type: "data",
  name: activityPartName(activity),
  data: payload,
});

describe("which parts are steps", () => {
  it("statuses but a failure, artifacts, checks, CI, reworks, actions and the markers", () => {
    expect(isStepPart(data(ACTIVITY.status, { status: "working" }))).toBe(true);
    expect(isStepPart(data(ACTIVITY.status, { status: "failed", detail: "x" }))).toBe(false);
    expect(isStepPart(data(ACTIVITY.artifact, { name: "x" }))).toBe(true);
    expect(isStepPart(data(ACTIVITY.check, {}))).toBe(true);
    expect(isStepPart(data(ACTIVITY.ci, {}))).toBe(true);
    expect(isStepPart(data(ACTIVITY.rework, {}))).toBe(true);
    expect(isStepPart(data(ACTIVITY.action, {}))).toBe(true);
    expect(isStepPart(data(ACTIVITY.job, {}))).toBe(true);
    expect(isStepPart({ type: "data", name: ACTOR_PART, data: {} })).toBe(true);
    // the words, an error and a surface stand on their own
    expect(isStepPart({ type: "text", text: "hello" })).toBe(false);
    expect(isStepPart(data(ACTIVITY.error, { message: "x" }))).toBe(false);
    expect(isStepPart(data(ACTIVITY.surface, { surface: "a" }))).toBe(false);
  });

  it("the statuses that come with the agent's words, and the markers, draw no line", () => {
    expect(drawsStep(data(ACTIVITY.status, { status: "completed" }))).toBe(false);
    expect(drawsStep(data(ACTIVITY.status, { status: "input_required" }))).toBe(false);
    expect(drawsStep(data(ACTIVITY.status, { status: "working" }))).toBe(true);
    expect(drawsStep(data(ACTIVITY.status, { status: "canceled" }))).toBe(true);
    expect(drawsStep(data(ACTIVITY.status, { status: "auth_required" }))).toBe(true);
    expect(drawsStep(data(ACTIVITY.job, { job: 2 }))).toBe(false);
    expect(drawsStep({ type: "data", name: ACTOR_PART, data: {} })).toBe(false);
    expect(drawsStep(data(ACTIVITY.artifact, { nope: 1 }))).toBe(false);
  });
});

describe("the words of a step", () => {
  it("a command is a working detail written as a shell prompt", () => {
    expect(commandOf("$ cargo test -p auth")).toBe("cargo test -p auth");
    expect(commandOf("  $   git push  ")).toBe("git push");
    expect(commandOf("Reading the code")).toBeUndefined();
    expect(commandOf("$ ")).toBeUndefined();
    expect(commandOf("costs $5")).toBeUndefined();
  });

  it("a repository is owner/name without its host", () => {
    expect(shortRepository("github.com/acme/demo")).toBe("acme/demo");
    expect(shortRepository("https://github.com/acme/demo.git")).toBe("acme/demo");
    expect(shortRepository("gitlab.example.com/group/sub/project")).toBe("group/sub/project");
    expect(shortRepository("demo")).toBe("demo");
  });

  it("the agent's checks: the summary and the findings of its JSON, nothing else", () => {
    expect(
      checksPayload(JSON.stringify({ passed: false, summary: "1 failed", findings: ["a", 2] })),
    ).toEqual({ summary: "1 failed", findings: ["a"] });
    expect(checksPayload("not json")).toEqual({ findings: [] });
    expect(checksPayload(undefined)).toEqual({ findings: [] });
    expect(checksPayload("null")).toEqual({ findings: [] });
  });

  it("a check of the gate says what its source does or said", () => {
    expect(checkLabel({ source: "ci", status: "pending", stale: false })).toBe("Waiting for CI");
    expect(checkLabel({ source: "ci", status: "failed", stale: false })).toBe("CI failed");
    expect(checkLabel({ source: "verifier", status: "passed", stale: false })).toBe(
      "The verifier approved the work",
    );
    expect(checkLabel({ source: "agent_checks", status: "failed", stale: false })).toBe(
      "The agent's checks failed",
    );
    expect(checkLabel({ source: "ci", status: "failed", stale: true })).toBe("A late CI answer");
    expect(checkLabel({ source: "lint_bot", status: "pending", stale: false })).toBe(
      "Waiting for lint bot",
    );
  });

  it("a rework names what sent the agent back and the attempt that starts", () => {
    expect(
      reworkLabel({
        attempt: 2,
        maxAttempts: 3,
        findings: [{ source: "agent_checks", findings: [] }],
      }),
    ).toBe("Checks failed — trying again (2/3)");
    expect(
      reworkLabel({ attempt: 3, maxAttempts: 3, findings: [{ source: "verifier", findings: [] }] }),
    ).toBe("The review found issues — trying again (3/3)");
    expect(
      reworkLabel({ attempt: 2, maxAttempts: 5, findings: [{ source: "ci", findings: [] }] }),
    ).toBe("CI failed — trying again (2/5)");
  });
});

describe("a pull request", () => {
  it("typed by the projection: its number, repository, branch and the title of its payload", () => {
    expect(
      pullRequestOf({
        kind: "pull_request",
        name: "pull_request",
        url: "https://github.com/acme/demo/pull/12",
        number: 12,
        repository: "github.com/acme/demo",
        branch: "agent/fix",
        text: JSON.stringify({
          title: "Fix the login",
          url: "https://github.com/acme/demo/pull/12",
        }),
      }),
    ).toEqual({
      href: "https://github.com/acme/demo/pull/12",
      label: "acme/demo#12",
      number: 12,
      repository: "acme/demo",
      branch: "agent/fix",
      title: "Fix the login",
    });
  });

  it("says where its link goes, whatever the payload claims", () => {
    // the payload says acme/demo#12; the link goes to evil.example
    expect(
      pullRequestOf({
        kind: "pull_request",
        name: "pull_request",
        url: "https://evil.example/acme/demo/pull/9",
        number: 12,
        repository: "github.com/acme/demo",
      }),
    ).toEqual({
      href: "https://evil.example/acme/demo/pull/9",
      label: "evil.example/acme/demo#9",
      number: 9,
      repository: "acme/demo",
    });
    // a URL that names no repository and number is labelled by its host
    expect(
      pullRequestOf({
        kind: "pull_request",
        name: "pull_request",
        url: "https://evil.example/review",
        number: 12,
        repository: "github.com/acme/demo",
      }),
    ).toEqual({ href: "https://evil.example/review", label: "evil.example" });
    // a link a browser would send elsewhere is none
    expect(
      pullRequestOf({
        kind: "pull_request",
        name: "pull_request",
        url: "https://github.com@evil.example/acme/demo/pull/1",
      }),
    ).toBeUndefined();
  });

  it("a typed one without a usable URL is none", () => {
    expect(pullRequestOf({ kind: "pull_request", name: "pull_request" })).toBeUndefined();
  });

  it("a file whose link is a pull request URL, with the file's text as a note", () => {
    expect(
      pullRequestOf({
        kind: "file",
        name: "result",
        uri: "https://github.com/acme/demo/pull/1",
        text: "echo: hi",
      }),
    ).toEqual({
      href: "https://github.com/acme/demo/pull/1",
      label: "acme/demo#1",
      number: 1,
      repository: "acme/demo",
      note: "echo: hi",
    });
    expect(pullRequestOf({ kind: "file", name: "notes", uri: "https://example.com/a" })).toBe(
      undefined,
    );
  });
});
