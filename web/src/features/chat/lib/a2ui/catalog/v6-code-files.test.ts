import { describe, expect, it } from "vitest";
import lock from "./catalog.lock.json";
import { componentNames, OWN_CATALOG } from "./index";
import { compileCatalog } from "./validate";

/**
 * Catalog version 6, group 1 (code and files): Code, Diff, FileCard, Terminal and PullRequest. Each
 * schema is checked on its own: what is valid, what is missing, what is oversize, and what has the
 * wrong type. The rules JSON Schema cannot say (a file of the thread, a link) are in prepare-v6.test.ts.
 */

const check = (component: string, instance: unknown) =>
  compileCatalog(OWN_CATALOG.catalog).check(component, instance);
const SHA = "a1".repeat(32);
const text = (n: number) => "x".repeat(n);

describe("version 6: code and files", () => {
  it("is catalog version 6, and has the five components", () => {
    expect(OWN_CATALOG.version).toBe(6);
    expect(lock.version).toBe(6);
    expect(componentNames(OWN_CATALOG.catalog)).toEqual(
      expect.arrayContaining(["Code", "Diff", "FileCard", "Terminal", "PullRequest"]),
    );
  });

  it("keeps the first sentence of each description short enough for adam's show to list it whole", () => {
    for (const name of ["Code", "Diff", "FileCard", "Terminal", "PullRequest"]) {
      const d = (OWN_CATALOG.catalog.components[name] as { description: string }).description;
      const end = d.indexOf(". ");
      const first = end < 0 ? d : d.slice(0, end + 1);
      expect(first.length, name).toBeLessThanOrEqual(100);
    }
  });
});

describe("Code", () => {
  const code = (over: Record<string, unknown> = {}) => ({
    id: "c",
    component: "Code",
    code: "const a = 1;",
    ...over,
  });

  it("accepts the smallest instance and one with every property", () => {
    expect(check("Code", code())).toBeUndefined();
    expect(
      check(
        "Code",
        code({ title: "Setup", filename: "src/a.ts", language: "typescript", code: "x" }),
      ),
    ).toBeUndefined();
  });

  it("needs its code: a missing one, an empty one and one that is not text are refused", () => {
    const { code: _code, ...noCode } = code();
    expect(check("Code", noCode)).toMatch(/required property "code"/);
    expect(check("Code", code({ code: "" }))).toMatch(/^code: /);
    expect(check("Code", code({ code: 5 }))).toMatch(/^code: /);
    expect(check("Code", code({ code: ["a"] }))).toMatch(/^code: /);
  });

  it("limits: 20,000 characters of code, 120 of title, 200 of filename, 40 of language", () => {
    expect(check("Code", code({ code: text(20_000) }))).toBeUndefined();
    expect(check("Code", code({ code: text(20_001) }))).toMatch(/^code: /);
    expect(check("Code", code({ title: text(120) }))).toBeUndefined();
    expect(check("Code", code({ title: text(121) }))).toMatch(/^title: /);
    expect(check("Code", code({ filename: text(200) }))).toBeUndefined();
    expect(check("Code", code({ filename: text(201) }))).toMatch(/^filename: /);
    expect(check("Code", code({ language: text(40) }))).toBeUndefined();
    expect(check("Code", code({ language: text(41) }))).toMatch(/^language: /);
  });

  it("has no member that could carry a URL or markup", () => {
    expect(check("Code", code({ html: "<b>x</b>" }))).toMatch(/html/);
    expect(check("Code", code({ url: "https://example.com" }))).toMatch(/url/);
  });
});

describe("Diff", () => {
  const diff = (over: Record<string, unknown> = {}) => ({
    id: "d",
    component: "Diff",
    diff: "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b",
    ...over,
  });

  it("accepts a unified diff with and without a title", () => {
    expect(check("Diff", diff())).toBeUndefined();
    expect(check("Diff", diff({ title: "Rename" }))).toBeUndefined();
  });

  it("needs its diff: missing, empty and not text are refused", () => {
    const { diff: _diff, ...noDiff } = diff();
    expect(check("Diff", noDiff)).toMatch(/required property "diff"/);
    expect(check("Diff", diff({ diff: "" }))).toMatch(/^diff: /);
    expect(check("Diff", diff({ diff: 1 }))).toMatch(/^diff: /);
  });

  it("limits: 20,000 characters of diff and 120 of title", () => {
    expect(check("Diff", diff({ diff: text(20_000) }))).toBeUndefined();
    expect(check("Diff", diff({ diff: text(20_001) }))).toMatch(/^diff: /);
    expect(check("Diff", diff({ title: text(121) }))).toMatch(/^title: /);
  });

  it("has no file member: the file names are in the diff's headers", () => {
    expect(check("Diff", diff({ filename: "x" }))).toMatch(/filename/);
  });
});

describe("FileCard", () => {
  const card = (over: Record<string, unknown> = {}) => ({
    id: "f",
    component: "FileCard",
    artifact: SHA,
    ...over,
  });

  it("names a file by its sha256 and nothing else", () => {
    expect(check("FileCard", card())).toBeUndefined();
    expect(check("FileCard", card({ artifact: "A1".repeat(32) }))).toMatch(/^artifact: /);
    expect(check("FileCard", card({ artifact: "a1".repeat(31) }))).toMatch(/^artifact: /);
    expect(check("FileCard", card({ artifact: "https://example.com/a.zip" }))).toMatch(
      /^artifact: /,
    );
    expect(check("FileCard", card({ artifact: 7 }))).toMatch(/^artifact: /);
  });

  it("needs the artifact, and has no URL member", () => {
    expect(check("FileCard", { id: "f", component: "FileCard" })).toMatch(
      /required property "artifact"/,
    );
    expect(check("FileCard", card({ url: "https://example.com/a.zip" }))).toMatch(/url/);
    expect(check("FileCard", card({ caption: "x" }))).toMatch(/caption/);
  });
});

describe("Terminal", () => {
  const term = (over: Record<string, unknown> = {}) => ({
    id: "t",
    component: "Terminal",
    command: "npm test",
    ...over,
  });

  it("accepts a command alone, and one with output and an exit code", () => {
    expect(check("Terminal", term())).toBeUndefined();
    expect(
      check("Terminal", term({ title: "Tests", output: "1 failed", exitCode: 1 })),
    ).toBeUndefined();
    expect(check("Terminal", term({ exitCode: -9 }))).toBeUndefined();
  });

  it("needs a command of 1 to 2,000 characters", () => {
    const { command: _command, ...noCommand } = term();
    expect(check("Terminal", noCommand)).toMatch(/required property "command"/);
    expect(check("Terminal", term({ command: "" }))).toMatch(/^command: /);
    expect(check("Terminal", term({ command: text(2000) }))).toBeUndefined();
    expect(check("Terminal", term({ command: text(2001) }))).toMatch(/^command: /);
  });

  it("limits the output to 20,000 characters, the exit code to a byte and an integer", () => {
    expect(check("Terminal", term({ output: text(20_000) }))).toBeUndefined();
    expect(check("Terminal", term({ output: text(20_001) }))).toMatch(/^output: /);
    expect(check("Terminal", term({ exitCode: 255 }))).toBeUndefined();
    expect(check("Terminal", term({ exitCode: 256 }))).toMatch(/^exitCode: /);
    expect(check("Terminal", term({ exitCode: -256 }))).toMatch(/^exitCode: /);
    expect(check("Terminal", term({ exitCode: 1.5 }))).toMatch(/^exitCode: /);
    expect(check("Terminal", term({ exitCode: "0" }))).toMatch(/^exitCode: /);
  });
});

describe("PullRequest", () => {
  const pr = (over: Record<string, unknown> = {}) => ({
    id: "p",
    component: "PullRequest",
    repo: "acme/demo",
    number: 42,
    title: "Add login",
    state: "open",
    ...over,
  });
  const checks = { passed: 3, failed: 1, pending: 2 };

  it("accepts the required members, and one with every property", () => {
    expect(check("PullRequest", pr())).toBeUndefined();
    expect(
      check(
        "PullRequest",
        pr({
          branch: "feature/login",
          checks,
          url: "https://github.com/acme/demo/pull/42",
        }),
      ),
    ).toBeUndefined();
  });

  it("takes exactly the four states, in lower case", () => {
    for (const state of ["open", "merged", "closed", "draft"]) {
      expect(check("PullRequest", pr({ state })), state).toBeUndefined();
    }
    for (const state of ["Open", "queued", "approved", ""]) {
      expect(check("PullRequest", pr({ state })), state).toMatch(/^state: /);
    }
  });

  it("needs its repository, number, title and state", () => {
    for (const key of ["repo", "number", "title", "state"]) {
      const rest: Record<string, unknown> = pr();
      delete rest[key];
      expect(check("PullRequest", rest), key).toMatch(new RegExp(`required property "${key}"`));
    }
  });

  it("the number is a positive integer; the repository 1 to 200 characters; the title 1 to 300", () => {
    expect(check("PullRequest", pr({ number: 0 }))).toMatch(/^number: /);
    expect(check("PullRequest", pr({ number: 1.5 }))).toMatch(/^number: /);
    expect(check("PullRequest", pr({ number: "42" }))).toMatch(/^number: /);
    expect(check("PullRequest", pr({ repo: text(200) }))).toBeUndefined();
    expect(check("PullRequest", pr({ repo: text(201) }))).toMatch(/^repo: /);
    expect(check("PullRequest", pr({ title: text(300) }))).toBeUndefined();
    expect(check("PullRequest", pr({ title: text(301) }))).toMatch(/^title: /);
    expect(check("PullRequest", pr({ title: "" }))).toMatch(/^title: /);
  });

  it("checks are all three counts, each a whole number from 0 to 10,000", () => {
    expect(check("PullRequest", pr({ checks: { passed: 1, failed: 0 } }))).toMatch(
      /required property "pending"/,
    );
    expect(check("PullRequest", pr({ checks: { ...checks, passed: -1 } }))).toMatch(
      /^checks\.passed: /,
    );
    expect(check("PullRequest", pr({ checks: { ...checks, failed: 10_001 } }))).toMatch(
      /^checks\.failed: /,
    );
    expect(check("PullRequest", pr({ checks: { ...checks, running: 1 } }))).toMatch(/running/);
  });

  it("a link is an http(s) URL of at most 2,048 characters", () => {
    expect(check("PullRequest", pr({ url: "https://github.com/a/b/pull/1" }))).toBeUndefined();
    expect(check("PullRequest", pr({ url: "javascript:alert(1)" }))).toMatch(/^url: /);
    expect(check("PullRequest", pr({ url: "/acme/demo/pull/42" }))).toMatch(/^url: /);
    expect(check("PullRequest", pr({ url: `https://e.example/${"p".repeat(2048 - 18)}` }))).toBe(
      undefined,
    );
    expect(check("PullRequest", pr({ url: `https://e.example/${"p".repeat(2049 - 18)}` }))).toMatch(
      /^url: /,
    );
  });
});
