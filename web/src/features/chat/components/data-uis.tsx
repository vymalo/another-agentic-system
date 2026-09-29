"use client";

import { makeAssistantDataUI } from "@assistant-ui/react";
import {
  ACTIVITY,
  ACTOR_PART,
  activityPartName,
  parseArtifact,
  parseError,
  parseStatus,
} from "@/features/chat/lib/agui/vymalo";
import { ArtifactCard } from "./parts/artifact-card";
import { ErrorLine } from "./parts/error-line";
import { StatusLine } from "./parts/status-line";

/*
 * One registered renderer per activity the orchestrator sends (docs/api/agui.md, "vymalo.*
 * schemas"). `@assistant-ui/react-ag-ui` turns an `ACTIVITY_SNAPSHOT` of `activityType` into the
 * data part `agui-activity/<activityType>`; assistant-ui hands the part's `{ name, data }` to
 * `render`, and the thread renders it through `part.dataRendererUI`. A shape a renderer does not
 * know renders nothing.
 */

const StatusDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.status),
  render: ({ data }) => {
    const status = parseStatus(data);
    return status ? <StatusLine data={status} /> : null;
  },
});

const ArtifactDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.artifact),
  render: ({ data }) => {
    const artifact = parseArtifact(data);
    return artifact ? <ArtifactCard data={artifact} /> : null;
  },
});

const ErrorDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.error),
  render: ({ data }) => {
    const error = parseError(data);
    return error ? <ErrorLine data={error} /> : null;
  },
});

/** The actor marker part is read by the message, not drawn. */
const ActorDataUI = makeAssistantDataUI<unknown>({
  name: ACTOR_PART,
  render: () => null,
});

/** `vymalo.action` (an A2UI action, ADR 0013) has no renderer until surfaces are rendered. */
const ActionDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.action),
  render: () => null,
});

/** Mounted once inside the runtime provider (chat-shell.tsx). */
export function DataUIs() {
  return (
    <>
      <StatusDataUI />
      <ArtifactDataUI />
      <ErrorDataUI />
      <ActorDataUI />
      <ActionDataUI />
    </>
  );
}
