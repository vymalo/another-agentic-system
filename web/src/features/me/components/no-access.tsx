import { PandaMark } from "@/components/brand/panda-mark";
import type { ApiMe } from "@/lib/api/types";

/**
 * The screen of a person whose roles grant nothing at all: every route of the orchestrator but
 * `GET /api/me` answers 403 `no_access` (ADR 0033), so there is nothing to show but who they are
 * signed in as, and what to do about it. It replaces the chat, which would only list errors.
 */
export function NoAccess({ me }: { me: ApiMe }) {
  const as = me.email ?? me.user;
  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-5 px-6 py-12 text-center">
      <PandaMark size={72} />
      <h1 className="text-[1.75rem] leading-tight font-medium tracking-tight text-balance">
        No access
      </h1>
      <p className="max-w-md text-[0.9375rem] text-muted-foreground text-balance">
        You are signed in as <strong className="font-medium text-foreground">{as}</strong>
        {me.name ? ` (${me.name})` : ""}, and none of your roles gives access to this app.
      </p>
      <p className="max-w-md text-sm text-muted-foreground text-balance">
        Ask whoever runs this deployment for a role, then reload this page. If you expected to have
        access, you may be signed in with the wrong account.
      </p>
    </main>
  );
}
