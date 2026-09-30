"use client";

import { makeAssistantDataUI } from "@assistant-ui/react";
import {
  ACTIVITY,
  ACTOR_PART,
  activityPartName,
  parseAction,
  parseArtifact,
  parseCheck,
  parseCi,
  parseError,
  parseRework,
  parseStatus,
} from "@/features/chat/lib/agui/vymalo";
import { ActionLine } from "./parts/action-line";
import { ArtifactCard } from "./parts/artifact-card";
import { CheckCard } from "./parts/check-card";
import { CiCard } from "./parts/ci-card";
import { ErrorLine } from "./parts/error-line";
import { ReworkDivider } from "./parts/rework-divider";
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

/** `vymalo.check`: a source of the verification gate answered for an attempt (ADR 0018). */
const CheckDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.check),
  render: ({ data }) => {
    const check = parseCheck(data);
    return check ? <CheckCard data={check} /> : null;
  },
});

/** `vymalo.ci`: a CI system reported a check on a commit (ADR 0017), replaced in place by its id. */
const CiDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.ci),
  render: ({ data }) => {
    const ci = parseCi(data);
    return ci ? <CiCard data={ci} /> : null;
  },
});

/** `vymalo.rework`: the gate failed and the agent is sent back (ADR 0018), a divider. */
const ReworkDataUI = makeAssistantDataUI<unknown>({
  name: activityPartName(ACTIVITY.rework),
  render: ({ data }) => {
    const rework = parseRework(data);
    return rework ? <ReworkDivider data={rework} /> : null;
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
      <CheckDataUI />
      <CiDataUI />
      <ReworkDataUI />
      <SurfaceDataUI />
    </>
  );
}
