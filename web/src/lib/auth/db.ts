import Dexie, { type Table } from "dexie";
import { DB_NAME } from "./constants";
import type { KeyRow, PendingRow, SessionRow } from "./types";

/*
 * Everything the web keeps about a sign-in (ADR 0054, decision 3), in IndexedDB through Dexie and
 * nowhere else: no localStorage, no cookie, no log. Three tables, see `types.ts`.
 */
export class AuthDb extends Dexie {
  keys!: Table<KeyRow, string>;
  session!: Table<SessionRow, string>;
  pending!: Table<PendingRow, string>;

  constructor() {
    super(DB_NAME);
    this.version(1).stores({
      keys: "id",
      session: "id",
      pending: "state, createdAt",
    });
  }
}

let db: AuthDb | undefined;

/** The database, opened on first use (never at import: a page that needs no token must not create it). */
export function authDb(): AuthDb {
  db ??= new AuthDb();
  return db;
}

/** Closes and forgets the handle, for tests. */
export function closeAuthDb() {
  db?.close();
  db = undefined;
}

/**
 * Whether the database exists, **without creating it**. A reader who never signed in (a public
 * share link) must leave no trace in IndexedDB, so the page asks the browser before it opens
 * anything. A browser that cannot list databases says yes: opening is then the only way to know.
 */
export async function authDbExists(): Promise<boolean> {
  if (typeof indexedDB === "undefined") return false;
  if (typeof indexedDB.databases !== "function") return true;
  try {
    return (await indexedDB.databases()).some((d) => d.name === DB_NAME);
  } catch {
    return true;
  }
}
