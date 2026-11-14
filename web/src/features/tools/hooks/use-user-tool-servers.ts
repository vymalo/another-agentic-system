import { useCallback, useEffect, useRef, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type {
  ApiCreateUserToolServer,
  ApiUpdateUserToolServer,
  ApiUserToolAuthorization,
  ApiUserToolServer,
} from "@/lib/api/types";
import { type Failure, failureOf, type Kind } from "../lib/user-servers";

/*
 * The person's own MCP servers (`/api/user-tool-servers`, ADR 0046; the recipe is in `docs/api/chat-api.yaml`, on
 * `listUserToolServers`). The list is read when the page opens and again every time the window regains focus (a
 * sign-in finished in another window changes a server's status where nothing here could see it). A write answers
 * with the server as it is now, which replaces its row; a write that failed, or whose outcome is not known, is
 * followed by a read. A secret is an argument of `add` and `update` and is held nowhere here: no state, no ref.
 */

export type UserServersState =
  | { status: "loading" }
  | {
      status: "ready";
      /** Sorted by name, as the API gives them. */
      servers: ApiUserToolServer[];
      /** The most one person holds (`UserToolServers.limit`). */
      limit: number;
      /** The kinds of authentication this deployment accepts; the form offers these and no others. */
      kinds: Kind[];
      /** A definition did not read back: the list is empty and the others are not lost (`error.code: unreadable`). */
      unreadable: boolean;
      /** Why the last read failed, beside the list it left on screen; null after a good read. */
      readError: string | null;
    }
  /** 404: the deployment has people's own servers off. A fact, not a failure. */
  | { status: "off" }
  /** 403: the person's roles grant nothing at all. */
  | { status: "no-access" }
  /** 401 that the app's own session handling did not take. */
  | { status: "signed-out" }
  | { status: "error"; message: string };

export type WriteResult<T = ApiUserToolServer> =
  | { ok: true; value: T }
  | { ok: false; failure: Failure };

export type UserToolServersView = {
  state: UserServersState;
  /** Reads the list again now (a failed first read, the person's "Check now"). */
  reload: () => void;
  add: (body: ApiCreateUserToolServer) => Promise<WriteResult>;
  /** `body.revision` is the one the row was read at. */
  update: (id: string, body: ApiUpdateUserToolServer) => Promise<WriteResult>;
  /** `deleteUserToolServer` takes no revision and no permission; a server that is already gone is deleted. */
  remove: (id: string) => Promise<WriteResult<null>>;
  /** `authorizeUserToolServer`: where to send the person. The answer is for the caller; nothing here keeps it. */
  authorize: (id: string) => Promise<WriteResult<ApiUserToolAuthorization>>;
};

type Problem = { status: number; code?: string; detail?: string };

const problemOf = (error: unknown, status: number): Problem => {
  const e = (error ?? {}) as { code?: unknown; detail?: unknown };
  return {
    status,
    ...(typeof e.code === "string" ? { code: e.code } : {}),
    ...(typeof e.detail === "string" ? { detail: e.detail } : {}),
  };
};

const byName = (a: ApiUserToolServer, b: ApiUserToolServer) =>
  a.name < b.name ? -1 : a.name > b.name ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0;

export function useUserToolServers(): UserToolServersView {
  const [state, setState] = useState<UserServersState>({ status: "loading" });
  const asked = useRef(0);
  const settled = useRef(0);
  const alive = useRef(true);
  const shown = useRef(false);
  const limit = useRef(8);
  const lastRead = useRef(0);

  const load = useCallback(() => {
    const mine = ++asked.current;
    lastRead.current = Date.now();
    // a read that is overtaken by a newer one is dropped, so a slow answer never replaces a fresher list
    const apply = (next: UserServersState) => {
      if (!alive.current || mine < settled.current) return;
      settled.current = mine;
      setState(next);
    };
    const later = (readError: string) => {
      if (!alive.current || mine < settled.current) return;
      settled.current = mine;
      setState((s) => (s.status === "ready" ? { ...s, readError } : s));
    };
    api
      .GET("/api/user-tool-servers")
      .then(({ data, error, response }) => {
        if (data) {
          shown.current = true;
          limit.current = data.limit;
          apply({
            status: "ready",
            servers: [...data.servers].sort(byName),
            limit: data.limit,
            kinds: data.kinds as Kind[],
            unreadable: data.error?.code === "unreadable",
            readError: null,
          });
        } else if (shown.current) {
          // a list on screen stays when a later read fails, whatever the failure: it is said beside it
          later(
            response.status === 401
              ? "You are signed out. Sign in again to see changes."
              : problemMessage(error),
          );
        } else if (response.status === 404) apply({ status: "off" });
        else if (response.status === 403) apply({ status: "no-access" });
        else if (response.status === 401) apply({ status: "signed-out" });
        else apply({ status: "error", message: problemMessage(error) });
      })
      .catch((e: unknown) => {
        if (shown.current) later(problemMessage(e));
        else apply({ status: "error", message: problemMessage(e) });
      });
  }, []);

  useEffect(() => {
    alive.current = true;
    load();
    // a tab that is switched to fires `visibilitychange` and `focus` together: one read, and none within a quarter second of another
    const again = () => {
      if (document.visibilityState === "hidden" || Date.now() - lastRead.current < 250) return;
      load();
    };
    window.addEventListener("focus", again);
    document.addEventListener("visibilitychange", again);
    return () => {
      alive.current = false;
      window.removeEventListener("focus", again);
      document.removeEventListener("visibilitychange", again);
    };
  }, [load]);

  const put = useCallback((server: ApiUserToolServer) => {
    // a read that began before this write may carry what it changed: it is dropped when it answers
    settled.current = ++asked.current;
    setState((s) =>
      s.status === "ready"
        ? { ...s, servers: [...s.servers.filter((x) => x.id !== server.id), server].sort(byName) }
        : s,
    );
  }, []);

  /** A refused or failed write, in words; the list is read again where the outcome is not what the person asked for. */
  const failed = useCallback(
    (problem: Problem, retryAfter: string | null): { ok: false; failure: Failure } => {
      const failure = failureOf(problem, { retryAfter, limit: limit.current });
      if (failure.reread) load();
      return { ok: false, failure };
    },
    [load],
  );
  const lost = useCallback(
    (): { ok: false; failure: Failure } => failed({ status: 0 }, null),
    [failed],
  );

  const add = useCallback<UserToolServersView["add"]>(
    async (body) => {
      try {
        const { data, error, response } = await api.POST("/api/user-tool-servers", { body });
        if (data) {
          put(data);
          return { ok: true, value: data };
        }
        return failed(problemOf(error, response.status), response.headers.get("retry-after"));
      } catch {
        return lost();
      }
    },
    [failed, lost, put],
  );

  const update = useCallback<UserToolServersView["update"]>(
    async (id, body) => {
      try {
        const { data, error, response } = await api.PATCH("/api/user-tool-servers/{serverId}", {
          params: { path: { serverId: id } },
          body,
        });
        if (data) {
          put(data);
          return { ok: true, value: data };
        }
        return failed(problemOf(error, response.status), response.headers.get("retry-after"));
      } catch {
        return lost();
      }
    },
    [failed, lost, put],
  );

  const remove = useCallback<UserToolServersView["remove"]>(
    async (id) => {
      try {
        const { error, response } = await api.DELETE("/api/user-tool-servers/{serverId}", {
          params: { path: { serverId: id } },
        });
        // a second delete is a 404: the server is gone, which is what was asked
        if (response.ok || response.status === 404) {
          settled.current = ++asked.current;
          setState((s) =>
            s.status === "ready" ? { ...s, servers: s.servers.filter((x) => x.id !== id) } : s,
          );
          return { ok: true, value: null };
        }
        return failed(problemOf(error, response.status), response.headers.get("retry-after"));
      } catch {
        return lost();
      }
    },
    [failed, lost],
  );

  const authorize = useCallback<UserToolServersView["authorize"]>(
    async (id) => {
      try {
        const { data, error, response } = await api.POST(
          "/api/user-tool-servers/{serverId}/authorize",
          { params: { path: { serverId: id } } },
        );
        if (data) return { ok: true, value: data };
        return failed(problemOf(error, response.status), response.headers.get("retry-after"));
      } catch {
        return lost();
      }
    },
    [failed, lost],
  );

  return { state, reload: load, add, update, remove, authorize };
}
