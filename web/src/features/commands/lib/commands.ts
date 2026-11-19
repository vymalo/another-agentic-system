/**
 * The commands an agent serves (ADR 0079, `docs/api/commands-v1.md`): the prompt templates a person
 * invokes with `/<name>`, read from the agent's live card as the capabilities document relays it.
 *
 * The orchestrator reads the card and carries the offer under the extension's URI; this module reads
 * that key as strictly as the contract has it (the same rules as the Rust reader,
 * `orch_core::commands::CommandsOffer::from_params`), and nothing else. **Nothing here sends
 * anything**: a command is the message's text, `/name <arguments>`, and the agent expands it where
 * the message arrives.
 */

/** The A2A extension an agent lists when it serves commands (docs/api/commands-v1.md, section 1). */
export const COMMANDS_URI = "https://agents.vymalo.com/a2a/extensions/commands/v1";

/**
 * One command of the offer: the agent's own name and description (untrusted card text, drawn as
 * text), and what follows the name, when the command takes arguments.
 */
export type AgentCommand = {
  /** The word a message invokes: `/name`. Not empty, without whitespace, never starting with `/`. */
  name: string;
  /** What the command does, for the person to read. */
  description: string;
  /** What follows the name, shown beside it in the menu (`<repo>`, a sentence); absent when the command takes nothing. */
  argumentHint?: string;
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** Whether `name` is one a message can invoke: not empty, without whitespace, not starting with the `/` that invokes it. */
const usableName = (name: string): boolean =>
  name !== "" && !name.startsWith("/") && !/\s/.test(name);

/**
 * `custom[COMMANDS_URI]` of the capabilities document, read as strictly as the contract has it:
 * `commands` a non-empty array, each entry an object with a `name` a message can invoke, a
 * `description` that is not blank, and an `argumentHint` that is a string when it is there at all
 * (`null` is no hint). A name listed twice keeps its **first** entry; unknown members are ignored;
 * the list is sorted by name. **Anything else is no offer at all** — not a shorter list — so the
 * menu is absent: the orchestrator never puts a malformed key there, and a client that got one fails
 * closed (ADR 0008).
 */
export function parseCommands(value: unknown): AgentCommand[] | null {
  if (!isRecord(value)) return null;
  const listed = value.commands;
  if (!Array.isArray(listed) || listed.length === 0) return null;
  const commands: AgentCommand[] = [];
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
  // unreachable while the list is not empty (its first entry is always kept); kept as the contract
  // says, so that the parser fails closed rather than answer with an empty offer
  if (commands.length === 0) return null;
  return commands.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
}

/** A `/name` typed as the whole message, from its first character. */
export type CommandQuery = { query: string };

/**
 * The trigger of the `/` menu: the text begins with `/` and holds no white space, so the message is
 * one `/name` (`/` alone is every command). A slash anywhere else, or a message with a space, is a
 * message and not a command.
 */
export function commandQueryOf(text: string): CommandQuery | null {
  if (!text.startsWith("/")) return null;
  const query = text.slice(1);
  return /\s/.test(query) ? null : { query };
}

/**
 * The commands a query offers: those whose name **starts with it**, case-insensitively, in the
 * order the document gives (by name). A query with a space is not a query and offers nothing. An
 * empty query is every command.
 */
export function matchingCommands<T extends Pick<AgentCommand, "name">>(
  commands: readonly T[],
  query: string,
): T[] {
  if (/\s/.test(query)) return [];
  const wanted = query.toLowerCase();
  return commands.filter((command) => command.name.toLowerCase().startsWith(wanted));
}

/** Whether a command takes arguments: its card gave a hint that says something. */
export const takesArguments = (command: Pick<AgentCommand, "argumentHint">): boolean =>
  (command.argumentHint ?? "").trim() !== "";

/**
 * The text a pick puts in the box: `/name` and, when the command takes arguments, a trailing space
 * for the person to type them after. The text form is what invokes the command; nothing is sent.
 */
export const insertionOf = (command: Pick<AgentCommand, "name" | "argumentHint">): string =>
  takesArguments(command) ? `/${command.name} ` : `/${command.name}`;
