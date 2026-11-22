import { takesSecret } from "@/features/tools/lib/user-servers";
import type { ApiUpdateUserToolServer, ApiUserToolServer } from "@/lib/api/types";

/*
 * A trusted component (ADR 0083, `docs/api/trusted-components-v1.md`): the
 * agent names one and the **client** draws it, with its own chrome. Nothing of it is a catalog component: the agent
 * cannot draw it, cannot change its words and never sees what the person types into it.
 *
 * This build draws one, `McpCredential` (a person's own MCP server credential, ADR 0046), because it is the only one
 * of the ADR's five whose API exists. A name outside the list is drawn as **words and a link**, never as a form
 * (decision 6): a component this build does not have is not approximated.
 */

/** The components this build draws. A card that asks for another one is words and a link. */
export const TRUSTED_COMPONENTS = ["McpCredential"] as const;
export type TrustedComponent = (typeof TRUSTED_COMPONENTS)[number];

export const isDrawn = (name: string): name is TrustedComponent =>
  (TRUSTED_COMPONENTS as readonly string[]).includes(name);

/**
 * What reaches the agent when the person acts: a **receipt**, never a value (decision 3, and §5 of the contract). It
 * rides in the action's `context`, the member the orchestrator already logs verbatim as `ui_action.context` and
 * forwards to the agent.
 *
 * `last4` is **omitted on purpose.** No route returns a suffix or a fingerprint of a stored secret (`UserToolServer`
 * says `hasCredential` and nothing else, and the credential broker answers no value), so the four characters the
 * contract allows would have to be cut from the secret **on the device**: that is the client deriving it, which the
 * ADR forbids ("the client sends the four characters it received from the API"). It arrives with the route that can
 * answer it. `version` and `where` belong to the vault's components (`SecretGenerate`, `SecretCollect`), which this
 * build does not draw.
 */
export type Receipt = {
  status: "created" | "updated";
  id: string;
  name: string;
  scopes?: string[];
};

/** The receipt of a server the API answered for: the person's own server, never a value of the secret. */
export function receiptOf(
  status: Receipt["status"],
  server: { id: string; name: string },
  scopes: readonly string[],
): Receipt {
  const kept = [...new Set(scopes)].filter((scope) => scope.length > 0);
  return {
    status,
    id: server.id,
    name: server.name,
    ...(kept.length > 0 ? { scopes: kept } : {}),
  };
}

/**
 * A new key for a server the person already has: the secret and the `revision` it was read at, and nothing else — the
 * contract's "`secret`, nothing else of the table" row of `updateUserToolServer`. The address, the kind and the name
 * are not sent, so a re-key can never move a server or forget what it was issued for.
 */
export const rekeyBody = (server: ApiUserToolServer, secret: string): ApiUpdateUserToolServer => ({
  secret,
  revision: server.revision,
});

/** The person's own servers that a key can be given to: the two kinds that take a secret. */
export const keyTakers = (servers: readonly ApiUserToolServer[]): ApiUserToolServer[] =>
  servers.filter((server) => takesSecret(server.auth.kind));

/** A name from an agent, as one short line of text: control characters become `?`, and it is cut. */
// biome-ignore lint/suspicious/noControlCharactersInRegex: stripping them is the point
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/g;
export const shownName = (name: string, max = 40): string =>
  name.replace(CONTROL, "?").slice(0, max);

/** The screen that adds a tool server by hand: where a card that cannot be drawn sends the person. */
export const TOOLS_HREF = "/tools";
