import { describe, expect, it } from "vitest";
import { COMMANDS_URI, readCommands } from "./commands";

/*
 * The mock's reading of `commands/v1` (docs/api/commands-v1.md section 1), held to the contract: the
 * same shape the orchestrator puts in the capabilities document, and nothing for a card that lists it
 * badly.
 */
describe("readCommands", () => {
  it("sorts by name, keeps the first entry of a repeated name, and drops a null hint", () => {
    expect(
      readCommands({
        commands: [
          { name: "zeta", description: "first zeta" },
          { name: "alpha", description: "only alpha", argumentHint: null },
          { name: "zeta", description: "second zeta", argumentHint: "<ignored>" },
        ],
        later: "ignored",
      }),
    ).toEqual([
      { name: "alpha", description: "only alpha" },
      { name: "zeta", description: "first zeta" },
    ]);
  });

  it("lists nothing for params that are not well formed", () => {
    for (const bad of [
      null,
      [],
      {},
      { commands: {} },
      { commands: [] },
      { commands: ["review"] },
      { commands: [null] },
      { commands: [{ description: "d" }] },
      { commands: [{ name: "", description: "d" }] },
      { commands: [{ name: "two words", description: "d" }] },
      { commands: [{ name: "/review", description: "d" }] },
      { commands: [{ name: "review" }] },
      { commands: [{ name: "review", description: " " }] },
      { commands: [{ name: "review", description: "d", argumentHint: 2 }] },
    ]) {
      expect(readCommands(bad), JSON.stringify(bad)).toBeNull();
    }
  });

  it("is the contract's URI", () => {
    expect(COMMANDS_URI).toBe("https://agents.vymalo.com/a2a/extensions/commands/v1");
  });
});
