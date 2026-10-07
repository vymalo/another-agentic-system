import { useRouter } from "next/navigation";
import { useCallback, useEffect, useState } from "react";
import { problemMessage, readerApi } from "@/lib/api/client";
import { redirectToSignIn } from "@/lib/api/session";
import { hadSession, rememberEdgeSession } from "@/lib/api/session-hint";
import { hasUsableSession } from "@/lib/auth/tokens";
import { type Reply, resolveShare, type SessionHint, type SharedView } from "../lib/resolve";

const LOADING: SharedView = { status: "loading" };
/**
 * Whether this browser has had a session: remembered (`session-hint.ts`, edge mode) or, in browser mode
 * (ADR 0054), a sign-in stored in IndexedDB, which is not a guess and is read without opening anything or
 * asking the network.
 */
async function had(): Promise<boolean> {
  if (hadSession()) return true;
  try {
    return await hasUsableSession();
  } catch {
    return false;
  }
}

const HINT: SessionHint = { had, set: (has) => void rememberEdgeSession(has) };

/** `GET /api/shared/{token}` or `/api/public/shared/{token}`, as the status and the body of one answer. */
async function get(token: string, route: "signed-in" | "public"): Promise<Reply> {
  const params = { params: { path: { token } } };
  const { data, error, response } =
    route === "public"
      ? await readerApi.GET("/api/public/shared/{token}", params)
      : await readerApi.GET("/api/shared/{token}", params);
  return {
    status: response.status,
    ...(data ? { data } : {}),
    ...(error ? { detail: problemMessage(error) } : {}),
  };
}

/**
 * The thread a share link names, for the page at `/s/<token>`: read once when the page opens (the
 * stream that follows says the rest, and a link that is taken down is a 404 on its reconnect). The
 * owner of the thread is sent to the thread itself, where their own routes are.
 */
export function useSharedThread(token: string): { view: SharedView; retry: () => void } {
  const router = useRouter();
  const [view, setView] = useState<SharedView>(LOADING);
  const [attempt, setAttempt] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: `attempt` is the retry's trigger
  useEffect(() => {
    let current = true;
    setView(LOADING);
    void resolveShare(token, (route) => get(token, route), redirectToSignIn, HINT).then((next) => {
      if (!current) return;
      if (next.status === "owner") router.replace(`/threads/${next.threadId}`);
      setView(next);
    });
    return () => {
      current = false;
    };
  }, [token, router, attempt]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  return { view, retry };
}
