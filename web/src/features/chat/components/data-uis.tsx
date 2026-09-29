"use client";

import { makeAssistantDataUI } from "@assistant-ui/react";
import {
  ACTIVITY,
  ACTOR_PART,
  activityPartName,
  parseAction,
  parseArtifact,
  parseError,
  parseStatus,
} from "@/features/chat/lib/agui/vymalo";
import { ActionLine } from "./parts/action-line";
import { ArtifactCard } from "./parts/artifact-card";
import { ErrorLine } from "./parts/error-line";
import { StatusLine } from "./parts/status-line";
import { SurfaceActivity } from "./surface/surface-activity";

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

/** `vymalo.action`: what the owner did on an A2UI surface (ADR 0013), a quiet line. */
const ActionDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.action),
  render: ({ data }) => {
    const action = parseAction(data);
    return action ? <ActionLine data={action} /> : null;
  },
});

/**
 * An A2UI surface (ADR 0013). `ThreadAgent` hands it over as this activity type, untouched, so
 * that it is validated (lib/a2ui/prepare.ts) before anything converts it.
 */
const SurfaceDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.surface),
  render: ({ data }) => <SurfaceActivity data={data} />,
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
      <SurfaceDataUI />
    </>
  );
}
