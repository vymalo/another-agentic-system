import { BrandMark } from "@/components/brand-mark";
import { cn } from "@/lib/utils";

/** Who the thread talks to: the agent's mark, its id and, when pinned, its release. */
export function AgentPill({
  agentId,
  release,
  className,
}: {
  agentId: string;
  release?: string | null | undefined;
  className?: string;
}) {
  return (
    <span
      data-slot="agent-pill"
      className={cn(
        "inline-flex h-7 max-w-full min-w-0 shrink-0 items-center gap-1.5 rounded-full border bg-background ps-1 pe-2.5 text-[0.8125rem]",
        className,
      )}
    >
      <BrandMark className="size-5" />
      <span className="truncate">
        <span className="font-medium capitalize">{agentId}</span>
        {release ? <span className="text-muted-foreground"> · {release}</span> : null}
      </span>
    </span>
  );
}
