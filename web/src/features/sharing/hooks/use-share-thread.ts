import { useCallback, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiShare, ApiShareLink, ApiThread } from "@/lib/api/types";
import type { ShareLevel } from "../lib/sharing";

export type ThreadSharer = {
  /** What the thread is shared as now, from the resource; undefined while it is private. */
  share: ApiShare | undefined;
  /** A request is out: the dialog's controls wait. */
  busy: boolean;
  /** Why the last change was refused, in the server's words. */
  error: string | null;
  /** What the last change did, for a screen reader (a new link: the old one is gone). */
  notice: string | null;
  /** `private` stops sharing (`DELETE`); another level shares, widens or narrows (`PUT`). Resolves whether it was done. */
  choose: (level: ShareLevel) => Promise<boolean>;
  /** A new link: the old one is a 404 from now on, and the thread stays shared as it was. */
  newLink: () => Promise<boolean>;
  /** Takes the link down: the thread is private again. Needs only ownership. */
  stop: () => Promise<boolean>;
};

const shareOf = (link: ApiShareLink): ApiShare => ({
  visibility: link.visibility,
  effective: link.effective,
  ...(link.url !== undefined ? { url: link.url } : {}),
});

/**
 * Sharing the open thread (ADR 0040): the owner's three calls, `PUT`, `DELETE` and `POST …/rotate`
 * of `/api/threads/{id}/share`. The answer is the thread's new `share`, handed on (`onChanged`) as the
 * thread the page holds, so the chip, the menu and the sidebar follow without another fetch. A
 * refusal (the cap, a role, a link that is gone) is the server's words and changes nothing.
 */
export function useShareThread(
  thread: ApiThread | null,
  onChanged: (thread: ApiThread) => void,
): ThreadSharer {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const threadId = thread?.id;

  const run = useCallback(
    async (call: (id: string) => Promise<void>): Promise<boolean> => {
      if (!threadId) return false;
      setBusy(true);
      setError(null);
      setNotice(null);
      try {
        await call(threadId);
        return true;
      } catch (e) {
        setError(problemMessage(e));
        return false;
      } finally {
        setBusy(false);
      }
    },
    [threadId],
  );

  const applyLink = useCallback(
    (link: ApiShareLink) => {
      if (thread) onChanged({ ...thread, share: shareOf(link) });
    },
    [thread, onChanged],
  );

  const stop = useCallback(
    () =>
      run(async (id) => {
        const { error: err, response } = await api.DELETE("/api/threads/{threadId}/share", {
          params: { path: { threadId: id } },
        });
        if (!response.ok) throw new Error(problemMessage(err));
        if (thread) {
          const { share: _gone, ...rest } = thread;
          onChanged(rest);
        }
        setNotice("Stopped sharing. The link no longer works.");
      }),
    [run, thread, onChanged],
  );

  const choose = useCallback(
    (level: ShareLevel) => {
      if (level === "private") return stop();
      if (thread?.share?.visibility === level) return Promise.resolve(true);
      return run(async (id) => {
        const { data, error: err } = await api.PUT("/api/threads/{threadId}/share", {
          params: { path: { threadId: id } },
          body: { visibility: level },
        });
        if (!data) throw new Error(problemMessage(err));
        applyLink(data);
      });
    },
    [run, stop, thread?.share?.visibility, applyLink],
  );

  const newLink = useCallback(
    () =>
      run(async (id) => {
        const { data, error: err } = await api.POST("/api/threads/{threadId}/share/rotate", {
          params: { path: { threadId: id } },
        });
        if (!data) throw new Error(problemMessage(err));
        applyLink(data);
        setNotice("New link made. The old link no longer works.");
      }),
    [run, applyLink],
  );

  return { share: thread?.share, busy, error, notice, choose, newLink, stop };
}
