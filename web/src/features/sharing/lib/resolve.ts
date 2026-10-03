import { problemMessage } from "@/lib/api/client";
import type { ApiSharedThread } from "@/lib/api/types";
import { isShareToken, type ShareSource } from "./sharing";

/**
 * What a shared page makes of its link (ADR 0040, section 12): the answer of `GET /api/shared/{token}`
 * and, when that is a 401, of `GET /api/public/shared/{token}`.
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
 * Reads a link. Signed in first: a 200 is an internal reader (or the owner), a 401 is a person who is
 * not signed in, who may still be a public reader; if that is a 404, the browser is sent to sign in
 * (`signIn` says whether it went: without a sign-in path built into the image it did not, and the
 * answer is the one neutral page). Any other refusal of a signed-in reader is that page too, and the
 * public route is not asked: a person who is signed in has nothing more to gain from it.
 */
export async function resolveShare(
  token: string,
  get: (route: "signed-in" | "public") => Promise<Reply>,
  signIn: () => boolean,
): Promise<SharedView> {
  if (!isShareToken(token)) return { status: "gone" };
  try {
    const first = await get("signed-in");
    if (first.status === 200 && first.data) {
      return first.data.isOwner
        ? { status: "owner", threadId: first.data.id }
        : { status: "ready", thread: first.data, source: { token, audience: "internal" } };
    }
    if (first.status !== 401) return answer(first);
    const second = await get("public");
    if (second.status === 200 && second.data) {
      return { status: "ready", thread: second.data, source: { token, audience: "public" } };
    }
    if (second.status === 404) return signIn() ? { status: "signing-in" } : { status: "gone" };
    return answer(second);
  } catch (e) {
    return { status: "error", message: problemMessage(e) };
  }
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
