"use client";

import { ArrowLeftIcon, PlusIcon } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { InlineStatus, LoadingStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import { useMe } from "@/features/me/hooks/use-me";
import { holds } from "@/features/me/lib/access";
import { TOOL_SERVERS_LABEL } from "@/features/session/components/account-menu";
import { openSignIn } from "@/lib/api/session";
import { useUserToolServers } from "../../hooks/use-user-tool-servers";
import { buildCreate, type ServerFields } from "../../lib/user-servers";
import { type FormOutcome, ServerForm } from "./server-form";
import { ServerRow } from "./server-row";

/**
 * `/tools`: the MCP servers a person adds for themselves (ADR 0046). The server enforces everything, so this offers
 * what it would accept: Add only to a holder of `tools.user.add` (`GET /api/me`) while they hold fewer than the
 * limit, and only the kinds of authentication the list says. Opened by its address without the permission, the page
 * says so in words and still shows what the person added, which they can delete; a deployment that has the feature
 * off (a 404) says that. The list is read again whenever the window regains focus, which is how a sign-in finished in
 * another window shows here.
 */
export function MyServersPage() {
  const view = useUserToolServers();
  const me = useMe();
  const [adding, setAdding] = useState(false);
  const [added, setAdded] = useState<string | null>(null);
  const person = me.status === "ready" ? me.me : null;
  // `me` unknown (it could not be read): nothing is hidden, and the server says no in words where the action was
  const mayChange = person === null || holds(person, "tools.user.add");
  const { state } = view;

  async function add(fields: ServerFields): Promise<FormOutcome> {
    setAdded(null);
    const result = await view.add(buildCreate(fields));
    if (!result.ok) {
      return {
        ok: false,
        message: result.failure.message,
        ...(result.failure.field ? { field: result.failure.field } : {}),
      };
    }
    setAdding(false);
    setAdded(result.value.name);
    return { ok: true };
  }

  return (
    <main className="mx-auto flex min-h-dvh w-full max-w-3xl flex-col gap-6 px-4 py-6 md:px-6">
      <header className="flex flex-col gap-3">
        <Button asChild variant="ghost" size="sm" className="-ms-2 self-start">
          <Link href="/">
            <ArrowLeftIcon aria-hidden="true" />
            Back to chat
          </Link>
        </Button>
        <h1 className="text-[1.75rem] leading-tight font-medium tracking-tight">
          {TOOL_SERVERS_LABEL}
        </h1>
        <p className="text-sm text-muted-foreground">
          MCP servers you add here are yours alone. Attach one to a chat from the Tools menu beside
          the message box, and the agent can use its tools in that chat.
        </p>
      </header>
      {state.status === "loading" ? <LoadingStatus>Loading your tool servers</LoadingStatus> : null}
      {state.status === "off" ? (
        <InlineStatus role="status">
          This deployment does not let people add tool servers of their own.
        </InlineStatus>
      ) : null}
      {state.status === "no-access" ? (
        <InlineStatus role="status">
          Your roles grant no access to this app. Ask whoever runs this deployment for a role.
        </InlineStatus>
      ) : null}
      {state.status === "signed-out" ? (
        <InlineStatus
          role="status"
          action={{
            label: "Sign in",
            // a deployment with no sign-in built in has nothing to open: reading again is all there is
            onClick: () => {
              if (openSignIn() === "none") view.reload();
            },
          }}
        >
          You are signed out. Sign in to see your tool servers.
        </InlineStatus>
      ) : null}
      {state.status === "error" ? (
        <InlineStatus
          tone="error"
          role="alert"
          action={{ label: "Try again", onClick: view.reload }}
        >
          {state.message}
        </InlineStatus>
      ) : null}
      {state.status === "ready" ? (
        <>
          {mayChange ? null : (
            <InlineStatus tone="warning" role="status">
              Your roles do not let you add or change tool servers of your own. You can still see
              the ones you added, and delete them.
            </InlineStatus>
          )}
          {state.readError ? (
            <InlineStatus
              tone="warning"
              role="status"
              action={{ label: "Try again", onClick: view.reload }}
            >
              The list could not be refreshed: {state.readError} What is shown may be out of date.
            </InlineStatus>
          ) : null}
          {state.unreadable ? (
            <InlineStatus tone="warning" role="status">
              One of your servers could not be read, so none are listed here. They are not lost: ask
              whoever runs this deployment.
            </InlineStatus>
          ) : null}
          {added ? <InlineStatus role="status">Added “{added}”.</InlineStatus> : null}
          <section aria-labelledby="my-servers-list" className="flex flex-col gap-3">
            <h2 id="my-servers-list" className="text-sm font-medium">
              Your servers{" "}
              <span className="font-normal text-muted-foreground">
                ({state.servers.length} of {state.limit})
              </span>
            </h2>
            {state.servers.length === 0 ? (
              <p className="text-sm text-muted-foreground">You have not added a server yet.</p>
            ) : (
              <ul aria-label="Your servers" className="flex flex-col gap-3">
                {state.servers.map((server) => (
                  <ServerRow
                    key={server.id}
                    server={server}
                    kinds={state.kinds}
                    canChange={mayChange}
                    actions={view}
                  />
                ))}
              </ul>
            )}
          </section>
          {mayChange ? (
            <section aria-labelledby="my-servers-add" className="flex flex-col gap-3">
              <h2 id="my-servers-add" className="text-sm font-medium">
                Add a server
              </h2>
              {state.servers.length >= state.limit ? (
                <p className="text-sm text-muted-foreground">
                  You have {state.limit} servers, the most one person can have. Delete one to add
                  another.
                </p>
              ) : adding || state.servers.length === 0 ? (
                <div className="rounded-xl border bg-card px-4 py-4">
                  <ServerForm
                    mode="add"
                    kinds={state.kinds}
                    onSubmit={add}
                    {...(state.servers.length > 0 ? { onCancel: () => setAdding(false) } : {})}
                  />
                </div>
              ) : (
                <Button
                  type="button"
                  variant="outline"
                  className="self-start"
                  onClick={() => {
                    setAdded(null);
                    setAdding(true);
                  }}
                >
                  <PlusIcon aria-hidden="true" />
                  Add a server
                </Button>
              )}
            </section>
          ) : null}
        </>
      ) : null}
    </main>
  );
}
