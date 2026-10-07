import { authDb } from "./db";
import type { KeyRow } from "./types";

/**
 * The DPoP key pair (ADR 0054, decision 3): ECDSA P-256, generated **non-extractable**, so the
 * private key can be used by this browser and never read, by a script or by anything that gets hold
 * of the database. The first caller makes it; a second tab that raced it finds the stored one.
 */
export async function dpopKeyPair(): Promise<CryptoKeyPair> {
  const db = authDb();
  const found = await db.keys.get("dpop");
  if (found) return found.pair;
  const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, [
    "sign",
    "verify",
  ]);
  const row: KeyRow = { id: "dpop", pair, createdAt: Date.now() };
  // generated outside the transaction (a transaction ends at a non-IndexedDB await): the first put wins
  return db.transaction("rw", db.keys, async () => {
    const raced = await db.keys.get("dpop");
    if (raced) return raced.pair;
    await db.keys.put(row);
    return pair;
  });
}

/** The stored pair, or undefined: never makes one. */
export async function storedKeyPair(): Promise<CryptoKeyPair | undefined> {
  return (await authDb().keys.get("dpop"))?.pair;
}
