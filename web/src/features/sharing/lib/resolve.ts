import { problemMessage } from "@/lib/api/client";
import type { ApiSharedThread } from "@/lib/api/types";
import { isShareToken, type ShareSource } from "./sharing";

/**
 * What a shared page makes of its link (ADR 0040, section 12): the answer of `GET /api/shared/{token}`
 * and of `GET /api/public/shared/{token}`, in the order the browser's session hint says.
 */
export type SharedView =
  | { status: "loading" }
  | { status: "ready"; thread: ApiSharedThread; source: ShareSource }
  /** The reader is the thread's owner: the thread itself is their page. */
  | { status: "owner"; threadId: string }
  /** Not signed in, and the link is not public: the browser is on its way to sign in. */
  | { status: "signing-in" }
  /** Every way a link fails, said once, without saying which. */
  | { status: "gone" }
  | { status: "error"; message: string };

/** One route's answer: the status, and the body of a 200 or the problem's words. */
export type Reply = { status: number; data?: ApiSharedThread; detail?: string };

const BUSY = "This link is being read a lot just now. Try again in a moment.";

/**
 * What the page remembers of the browser's session (`lib/api/session-hint.ts`): only which route is
 * asked first. `had` says whether this browser has been signed in; `set` is told what a signed-in
 * route found (a 200 is a session, a 401 is none).
 */
export type SessionHint = {
  had: () => boolean | Promise<boolean>;
  set: (has: boolean) => void;
};

const ALWAYS_SIGNED_IN: SessionHint = { had: () => true, set: () => {} };

type Get = (route: "signed-in" | "public") => Promise<Reply>;

/**
 * Reads a link. Which route comes first depends on the hint, and the other is always the fall back.
 *
 * A browser that has **had a session** asks the signed-in route first: a 200 is an internal reader
 * (or the owner), a 401 is a person who is not signed in after all, who may still be a public reader,
 * and the public route is asked.
 *
 * A browser that has **never had one** (a visitor who followed a link) asks the public route first, so
 * the edge's 401 of the signed-in route is not met just to be refused. A 200 reads the link as anybody
 * (no token is sent: ADR 0040). A 404 is a link that is not public, or a signed-in person whose
 * browser forgot: the signed-in route is asked, and a 401 there is the one place the visitor has to
 * sign in.
 *
 * Whichever way, when no route finds the reader, the browser is sent to sign in (`signIn` says whether
 * it went: without a sign-in path built into the image it did not, and the answer is the one neutral
 * page). Any other refusal of a signed-in reader is that page too, and a refusal of the public route
 * other than 404 is its own answer: a person who is signed in has nothing more to gain from asking again.
 */
export async function resolveShare(
  token: string,
  get: Get,
  signIn: () => boolean,
  hint: SessionHint = ALWAYS_SIGNED_IN,
): Promise<SharedView> {
  if (!isShareToken(token)) return { status: "gone" };
  try {
    if (await hint.had()) {
      const view = await signedInRoute(token, get, hint);
      if (view !== "unauthenticated") return view;
      const reply = await get("public");
      if (reply.status === 404) return signInOrGone(signIn);
      return publicAnswer(token, reply);
    }
    const reply = await get("public");
    if (reply.status !== 404) return publicAnswer(token, reply);
    const view = await signedInRoute(token, get, hint);
    return view === "unauthenticated" ? signInOrGone(signIn) : view;
  } catch (e) {
    return { status: "error", message: problemMessage(e) };
  }
}

/** The signed-in route's verdict, or `unauthenticated` for a 401 (which the hint forgets). */
async function signedInRoute(
  token: string,
  get: Get,
  hint: SessionHint,
): Promise<SharedView | "unauthenticated"> {
  const first = await get("signed-in");
  if (first.status === 200 && first.data) {
    hint.set(true);
    return first.data.isOwner
      ? { status: "owner", threadId: first.data.id }
      : { status: "ready", thread: first.data, source: { token, audience: "internal" } };
  }
  if (first.status === 401) {
    hint.set(false);
    return "unauthenticated";
  }
  return answer(first);
}

const signInOrGone = (signIn: () => boolean): SharedView =>
  signIn() ? { status: "signing-in" } : { status: "gone" };

/** The public route's answer other than a 404: anybody reads a public link; anything else is the neutral page or its words. */
function publicAnswer(token: string, reply: Reply): SharedView {
  if (reply.status === 200 && reply.data) {
    return { status: "ready", thread: reply.data, source: { token, audience: "public" } };
  }
  return answer(reply);
}

/** A reply that is not a thread: the neutral page for a refusal, the wait for a limit, else the error. */
function answer(reply: Reply): SharedView {
  if (reply.status === 404 || reply.status === 403) return { status: "gone" };
  if (reply.status === 429) return { status: "error", message: BUSY };
  return {
    status: "error",
    message: reply.detail ?? "Could not load the conversation. Try again.",
  };
}
