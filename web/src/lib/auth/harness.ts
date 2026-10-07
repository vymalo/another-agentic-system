import Dexie from "dexie";
import { IDBFactory } from "fake-indexeddb";
import { vi } from "vitest";
import { resetClock } from "./clock";
import { setBrowserAuth } from "./config";
import { closeAuthDb } from "./db";
import { navigation } from "./navigation";
import { resetDiscovery } from "./oidc";
import { completeSignIn, startSignIn } from "./sign-in";
import { CLIENT_ID, type FakeIssuer, fakeIssuer, ISSUER, ORIGIN, type User } from "./test-issuer";

/** The page of a test: a window with an origin, an IndexedDB of its own, a fake issuer for `fetch`. */
export type Page = {
  issuer: FakeIssuer;
  /** Every URL the page was sent to. */
  went: string[];
};

export const ALICE: User = { sub: "sub-alice", email: "alice@example.test" };
export const BOB: User = { sub: "sub-bob", email: "bob@example.test" };

export function openPage(): Page {
  closeAuthDb();
  const factory = new IDBFactory();
  Dexie.dependencies.indexedDB = factory;
  vi.stubGlobal("indexedDB", factory);
  resetClock();
  resetDiscovery();
  const issuer = fakeIssuer();
  vi.stubGlobal("window", {
    location: { origin: ORIGIN, pathname: "/threads/1", search: "", hash: "" },
  });
  vi.stubGlobal("fetch", issuer.fetch);
  setBrowserAuth({
    issuer: ISSUER,
    clientId: CLIENT_ID,
    scope: "openid email profile offline_access",
  });
  const went: string[] = [];
  vi.spyOn(navigation, "go").mockImplementation((url) => void went.push(url));
  return { issuer, went };
}

/** Runs the whole sign-in as `user`: the redirect, the approval, the callback. */
export async function signInAs(page: Page, user: User, returnTo = "/threads/9") {
  await startSignIn({ returnTo });
  const url = new URL(page.went.at(-1) as string);
  const state = url.searchParams.get("state") as string;
  const code = page.issuer.approve(user, url.searchParams.get("code_challenge") as string);
  return completeSignIn(`${ORIGIN}/auth/callback?code=${code}&state=${state}`);
}
