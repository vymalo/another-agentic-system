import type { ThreadView } from "@/chat/use-thread";
import { StateBadge } from "./StateBadge";

export function ThreadHeader({ view }: { view: ThreadView }) {
  const { thread, state, connection } = view;
  const target = thread
    ? [thread.target.agentId, thread.target.release].filter(Boolean).join(" · ")
    : null;
  return (
    <header className="header">
      <div className="header__text">
        <h1 className="header__title">{thread?.title ?? "Loading thread…"}</h1>
        {target ? <p className="header__meta">{target}</p> : null}
      </div>
      <div className="header__aside">
        {connection === "reconnecting" ? (
          <span className="header__conn" role="status">
            Reconnecting…
          </span>
        ) : null}
        <StateBadge state={state} />
      </div>
    </header>
  );
}
