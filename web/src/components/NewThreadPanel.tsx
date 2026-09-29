import type { AgentsView } from "@/chat/use-agents";
import type { Selection } from "@/chat/use-chat-runtime";
import { InlineStatus } from "./InlineStatus";

type Props = {
  agents: AgentsView;
  selection: Selection;
  onSelect: (s: Selection) => void;
};

/** Agent picker, plus a release dropdown only when the selected agent offers `releases`. */
export function NewThreadPanel({ agents, selection, onSelect }: Props) {
  const { agents: list, loading, error, retry } = agents;
  if (loading && list.length === 0)
    return <InlineStatus role="status">Loading agents…</InlineStatus>;
  if (error && list.length === 0) {
    return (
      <InlineStatus tone="error" role="alert" action={{ label: "Retry", onClick: retry }}>
        Could not load agents: {error}
      </InlineStatus>
    );
  }
  if (list.length === 0) return <InlineStatus>No agents are configured.</InlineStatus>;

  const agent = list.find((a) => a.id === selection.agentId) ?? list[0];
  const releases = agent?.releases;
  const channels = releases ? Object.entries(releases.channels) : [];
  const release =
    releases && selection.release && isKnown(releases, selection.release)
      ? selection.release
      : releases?.defaultChannel;

  return (
    <section className="new-thread" aria-label="New thread">
      <h1 className="new-thread__title">New thread</h1>
      <div className="fields">
        <div className="field">
          <label htmlFor="agent">Agent</label>
          <select
            id="agent"
            value={agent?.id ?? ""}
            onChange={(e) => onSelect({ agentId: e.target.value, release: null })}
          >
            {list.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
          {agent?.description ? <p className="field__hint">{agent.description}</p> : null}
        </div>
        {releases ? (
          <div className="field">
            <label htmlFor="release">Release</label>
            <select
              id="release"
              value={release ?? ""}
              onChange={(e) => onSelect({ agentId: agent?.id ?? null, release: e.target.value })}
            >
              {channels.map(([channel, revision]) => (
                <option key={channel} value={channel}>
                  {channel} — {revision}
                </option>
              ))}
              {releases.revisions?.length ? (
                <optgroup label="Revisions">
                  {releases.revisions.map((rev) => (
                    <option key={rev} value={rev}>
                      {rev}
                    </option>
                  ))}
                </optgroup>
              ) : null}
            </select>
          </div>
        ) : null}
      </div>
    </section>
  );
}

function isKnown(
  releases: NonNullable<Props["agents"]["agents"][number]["releases"]>,
  value: string,
) {
  return value in releases.channels || (releases.revisions ?? []).includes(value);
}
