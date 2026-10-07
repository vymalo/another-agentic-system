/** The channel the sign-in popup's last page calls on, and the tabs listen to. */
export const SIGNED_IN_CHANNEL = "another-agentic.signed-in";
/** The one Web Lock every tab takes to spend a refresh token (ADR 0054, decision 4). */
export const REFRESH_LOCK = "another-agentic.auth.refresh";
/** The IndexedDB database that holds the keys, the session and the sign-ins under way. */
export const DB_NAME = "another-agentic-auth";
/** Where the issuer sends the person back to: a page of this app that finishes the sign-in. */
export const CALLBACK_PATH = "/auth/callback";
/** A refresh happens when the access token has less than this left (decision 4). */
export const REFRESH_SKEW_MS = 60_000;
/** A sign-in under way is forgotten after this (decision 3). */
export const PENDING_TTL_MS = 10 * 60_000;
