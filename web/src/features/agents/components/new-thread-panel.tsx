import { InlineStatus, LoadingStatus } from "@/components/inline-status";
import { Label } from "@/components/ui/label";
import {
  NativeSelect,
  NativeSelectOptGroup,
  NativeSelectOption,
} from "@/components/ui/native-select";
import type { AgentsView } from "@/features/agents/hooks/use-agents";
import type { Selection } from "@/features/chat/hooks/use-chat-runtime";

type Props = {
  agents: AgentsView;
  selection: Selection;
  onSelect: (s: Selection) => void;
};

/** Native selects: the best picker on a phone, and they group revisions in an `<optgroup>`. */
const SELECT = "w-full sm:w-64 [&_select]:h-11 sm:[&_select]:h-9";

/** Agent picker, plus a release dropdown only when the selected agent offers `releases`. */
export function NewThreadPanel({ agents, selection, onSelect }: Props) {
  const { agents: list, loading, error, retry } = agents;
  if (loading && list.length === 0)
    return <LoadingStatus className="pt-4">Loading agents…</LoadingStatus>;
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
    <section aria-label="New thread">
      <h1 className="pt-4 pb-3 text-lg font-semibold">New thread</h1>
      <div className="flex flex-wrap gap-x-6 gap-y-4">
        <div className="flex min-w-50 flex-col gap-1.5 max-sm:flex-1 max-sm:basis-full">
          <Label htmlFor="agent">Agent</Label>
          <NativeSelect
            id="agent"
            className={SELECT}
            value={agent?.id ?? ""}
            onChange={(e) => onSelect({ agentId: e.target.value, release: null })}
          >
            {list.map((a) => (
              <NativeSelectOption key={a.id} value={a.id}>
                {a.name}
              </NativeSelectOption>
            ))}
          </NativeSelect>
          {agent?.description ? (
            <p className="max-w-xs text-[0.8125rem] text-muted-foreground">{agent.description}</p>
          ) : null}
        </div>
        {releases ? (
          <div className="flex min-w-50 flex-col gap-1.5 max-sm:flex-1 max-sm:basis-full">
            <Label htmlFor="release">Release</Label>
            <NativeSelect
              id="release"
              className={SELECT}
              value={release ?? ""}
              onChange={(e) => onSelect({ agentId: agent?.id ?? null, release: e.target.value })}
            >
              {channels.map(([channel, revision]) => (
                <NativeSelectOption key={channel} value={channel}>
                  {channel} — {revision}
                </NativeSelectOption>
              ))}
              {releases.revisions?.length ? (
                <NativeSelectOptGroup label="Revisions">
                  {releases.revisions.map((rev) => (
                    <NativeSelectOption key={rev} value={rev}>
                      {rev}
                    </NativeSelectOption>
                  ))}
                </NativeSelectOptGroup>
              ) : null}
            </NativeSelect>
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
