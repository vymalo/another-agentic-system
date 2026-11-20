import { describe, expect, it } from "vitest";
import {
  type AgentCommand,
  COMMANDS_URI,
  commandQueryOf,
  insertionOf,
  matchingCommands,
  parseCommands,
  takesArguments,
} from "./commands";

/*
 * `commands/v1` read from the capabilities document (ADR 0079, docs/api/commands-v1.md section 1):
 * the same rules as the Rust reader, case by case, and the prefix match a `/` menu uses.
 */

const good = {
  commands: [
    { name: "review", description: "Reviews a change.", argumentHint: "<repo>" },
    { name: "explain", description: "Explains a file." },
    { name: "test", description: "Runs the tests." },
  ],
  later: "ignored",
};

const names = (commands: readonly AgentCommand[] | null) => commands?.map((c) => c.name);

describe("parseCommands", () => {
  it("reads the offer, sorted by name, with a hint only where the card has one", () => {
    const commands = parseCommands(good);
    expect(names(commands)).toEqual(["explain", "review", "test"]);
    expect(commands?.find((c) => c.name === "review")).toEqual({
      name: "review",
      description: "Reviews a change.",
      argumentHint: "<repo>",
    });
    expect(commands?.find((c) => c.name === "explain")).not.toHaveProperty("argumentHint");
  });

  it("keeps the first entry of a name listed twice, and still sorts", () => {
    const commands = parseCommands({
      commands: [
        { name: "zeta", description: "first zeta" },
        { name: "alpha", description: "only alpha" },
        { name: "zeta", description: "second zeta", argumentHint: "<ignored>" },
      ],
    });
    expect(names(commands)).toEqual(["alpha", "zeta"]);
    expect(commands?.find((c) => c.name === "zeta")).toEqual({
      name: "zeta",
      description: "first zeta",
    });
  });

  it("reads an argumentHint that is null as no hint", () => {
    const commands = parseCommands({
      commands: [{ name: "a", description: "d", argumentHint: null }],
    });
    expect(commands).toEqual([{ name: "a", description: "d" }]);
  });

  it("offers nothing for params that are not well formed (the whole list, never half of it)", () => {
    const bad: unknown[] = [
      null,
      undefined,
      "commands",
      [],
      {},
      { commands: {} },
      { commands: [] },
      { commands: ["review"] },
      { commands: [null] },
      { commands: [{ description: "no name" }] },
      { commands: [{ name: "", description: "d" }] },
      { commands: [{ name: 3, description: "d" }] },
      { commands: [{ name: "two words", description: "d" }] },
      { commands: [{ name: "tab\there", description: "d" }] },
      { commands: [{ name: "/review", description: "d" }] },
      { commands: [{ name: "review" }] },
      { commands: [{ name: "review", description: "" }] },
      { commands: [{ name: "review", description: "  " }] },
      { commands: [{ name: "review", description: 1 }] },
      { commands: [{ name: "review", description: "d", argumentHint: 2 }] },
      {
        commands: [
          { name: "ok", description: "d" },
          { name: "", description: "d" },
        ],
      },
    ];
    for (const value of bad) expect(parseCommands(value), JSON.stringify(value)).toBeNull();
  });

  it("is the contract's URI, spelled exactly", () => {
    expect(COMMANDS_URI).toBe("https://agents.vymalo.com/a2a/extensions/commands/v1");
  });
});

describe("commandQueryOf", () => {
  it("is the text from its first character, when it is one /word", () => {
    expect(commandQueryOf("/rev")).toEqual({ query: "rev" });
    expect(commandQueryOf("/")).toEqual({ query: "" });
  });

  it("is not a query with a space in it, nor a slash anywhere else", () => {
    expect(commandQueryOf("/review acme")).toBeNull();
    expect(commandQueryOf("/rev ")).toBeNull();
    expect(commandQueryOf("ask /rev")).toBeNull();
    expect(commandQueryOf("")).toBeNull();
    expect(commandQueryOf("review")).toBeNull();
  });
});

describe("matchingCommands", () => {
  const commands = parseCommands(good) ?? [];

  it("matches the start of a name, whatever the case, in the document's order", () => {
    expect(names(matchingCommands(commands, "re"))).toEqual(["review"]);
    expect(names(matchingCommands(commands, "RE"))).toEqual(["review"]);
    expect(names(matchingCommands(commands, "t"))).toEqual(["test"]);
    // `re` is a prefix of one name and inside another: only the prefix counts
    expect(names(matchingCommands(commands, "ev"))).toEqual([]);
  });

  it("offers every command for an empty query, and none for a query with a space", () => {
    expect(names(matchingCommands(commands, ""))).toEqual(["explain", "review", "test"]);
    expect(matchingCommands(commands, "rev ")).toEqual([]);
    expect(matchingCommands(commands, "review acme")).toEqual([]);
  });

  it("offers nothing when no name matches", () => {
    expect(matchingCommands(commands, "zzz")).toEqual([]);
    expect(matchingCommands([], "re")).toEqual([]);
  });
});

describe("what a pick inserts", () => {
  it("is /name and a space when the command takes arguments, /name alone otherwise", () => {
    expect(insertionOf({ name: "review", argumentHint: "<repo>" })).toBe("/review ");
    expect(insertionOf({ name: "explain" })).toBe("/explain");
    // a hint that says nothing is no hint: no space, and nothing drawn beside the name
    expect(insertionOf({ name: "explain", argumentHint: "" })).toBe("/explain");
    expect(insertionOf({ name: "explain", argumentHint: "   " })).toBe("/explain");
    expect(takesArguments({ argumentHint: "<repo>" })).toBe(true);
    expect(takesArguments({})).toBe(false);
  });
});
