"use client";

import { makeAssistantDataUI } from "@assistant-ui/react";
import {
  ACTIVITY,
  ACTOR_PART,
  activityPartName,
  parseError,
  parseStatus,
} from "@/features/chat/lib/agui/vymalo";
import { ErrorCallout, FailedCallout } from "./parts/error-callout";
import { SurfaceActivity } from "./surface/surface-activity";

/*
 * The renderers of the activities that stand on their own in a turn (docs/api/agui.md, "vymalo.*
 * schemas"). `@assistant-ui/react-ag-ui` turns an `ACTIVITY_SNAPSHOT` of `activityType` into the
 * data part `agui-activity/<activityType>`; assistant-ui hands the part's `{ name, data }` to
 * `render`, and the thread renders it through `part.dataRendererUI`.
 *
 * The activities that are steps (a working status, an artifact, a check, a CI report, a rework, an
 * action, a step of steps/v1) are not drawn here, nor in the chat: the thread groups them and
 * draws nothing for the group, and the side panel's step tree (`lib/step-tree.ts`,
 * `steps/steps-pane.tsx`) is where they are. A shape a renderer does not know renders nothing.
 */

/** A status that reaches a leaf is a failure (lib/steps.ts keeps the others in the panel's tree). */
const StatusDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.status),
  render: ({ data }) => {
    const status = parseStatus(data);
    return status?.status === "failed" ? <FailedCallout data={status} /> : null;
  },
});

const ErrorDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.error),
  render: ({ data }) => {
    const error = parseError(data);
    return error ? <ErrorCallout data={error} /> : null;
  },
});

/** The actor marker part is read by the message, not drawn. */
const ActorDataUI = makeAssistantDataUI<unknown>({
  name: ACTOR_PART,
  render: () => null,
});

/**
 * `vymalo.job`: a message on a finished thread started the thread's next job (ADR 0020). The
 * message itself is the boundary a person sees, so the marker draws nothing.
 */
const JobDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.job),
  render: () => null,
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
      <ErrorDataUI />
      <ActorDataUI />
      <JobDataUI />
      <SurfaceDataUI />
    </>
  );
}
