import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardHeader } from "@/components/ui/card";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { safeLinkHref } from "@/features/chat/lib/artifact";
import type { ArtifactPartData } from "@/features/chat/lib/to-items";
import { ActorLabel } from "../actor-label";

const LINK = "h-auto justify-start px-0 font-semibold [overflow-wrap:anywhere] whitespace-normal";

/** An artifact: a pull request becomes a link card, other URIs a link, inline text a disclosure. */
export function ArtifactCard({ data }: { data: ArtifactPartData }) {
  const href = safeLinkHref(data.uri);
  return (
    <Card
      size="sm"
      role="region"
      aria-label={`Artifact: ${data.name}`}
      className="w-full max-w-md shadow-none"
    >
      <CardHeader>
        <Badge variant="secondary" className="uppercase tracking-wide">
          Artifact
        </Badge>
        <CardAction>
          <ActorLabel actor={data.actor} />
        </CardAction>
      </CardHeader>
      <CardContent className="gap-2">
        {data.pr ? (
          <Button asChild variant="link" className={LINK}>
            <a href={data.pr.href} target="_blank" rel="noopener noreferrer">
              Pull request {data.pr.label}
            </a>
          </Button>
        ) : href ? (
          <Button asChild variant="link" className={LINK}>
            <a href={href} target="_blank" rel="noopener noreferrer">
              Open {data.name}
            </a>
          </Button>
        ) : (
          <p className="font-semibold [overflow-wrap:anywhere]">
            {data.name}
            {data.uri ? (
              <span className="text-xs font-normal text-muted-foreground"> ({data.uri})</span>
            ) : null}
          </p>
        )}
        {data.text ? (
          <Collapsible className="group/artifact-text">
            <CollapsibleTrigger className="cursor-pointer rounded-sm text-sm underline underline-offset-2 outline-none focus-visible:ring-3 focus-visible:ring-ring/50">
              {data.name}
            </CollapsibleTrigger>
            {/* Kept in the DOM while closed (hidden), like a <details> body. */}
            <CollapsibleContent forceMount className="hidden data-[state=open]:block">
              <pre className="mt-2 max-h-80 overflow-auto rounded-md bg-muted p-2 text-[0.8125rem] whitespace-pre-wrap [overflow-wrap:anywhere]">
                {data.text}
              </pre>
            </CollapsibleContent>
          </Collapsible>
        ) : null}
        {data.mimeType ? <p className="text-xs text-muted-foreground">{data.mimeType}</p> : null}
      </CardContent>
    </Card>
  );
}
