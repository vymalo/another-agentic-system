/**
 * `commands/v1` as the mock reads it (docs/api/commands-v1.md, ADR 0079), written from the contract
 * and not from the web's own parser (`src/features/commands`), so that the two are two readings of
 * one document: the web's tests and the contract's do not agree by sharing code.
 */

/** The URI an agent lists on its card, and the key of the capabilities document's `custom` under it. */
export const COMMANDS_URI = "https://agents.vymalo.com/a2a/extensions/commands/v1";

/** One command of a card's `params.commands`. */
export type Command = { name: string; description: string; argumentHint?: string };

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** Not empty, without whitespace, and never starting with the `/` that invokes it. */
const usableName = (name: string): boolean =>
  name !== "" && !name.startsWith("/") && !/\s/.test(name);

/**
 * `params` of a card's entry: `{commands: [{name, description, argumentHint?}]}`, sorted by name and
 * each name once (the first entry of a repeated one wins). `null` for anything else: a card that
 * lists the extension badly does not list it, and the capabilities document has no key.
 */
export function readCommands(params: unknown): Command[] | null {
  if (!isRecord(params)) return null;
  const listed = params.commands;
  if (!Array.isArray(listed) || listed.length === 0) return null;
  const commands: Command[] = [];
  for (const entry of listed) {
    if (!isRecord(entry)) return null;
    const { name, description, argumentHint } = entry;
    if (typeof name !== "string" || !usableName(name)) return null;
    if (typeof description !== "string" || description.trim() === "") return null;
    if (argumentHint !== undefined && argumentHint !== null && typeof argumentHint !== "string") {
      return null;
    }
    if (commands.some((c) => c.name === name)) continue;
    commands.push({
      name,
      description,
      ...(typeof argumentHint === "string" ? { argumentHint } : {}),
    });
  }
  return commands.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
}
