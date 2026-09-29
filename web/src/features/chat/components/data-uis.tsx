"use client";

import { makeAssistantDataUI } from "@assistant-ui/react";
import type { ArtifactPartData, ErrorPartData, StatusPartData } from "@/features/chat/lib/to-items";
import { ArtifactCard } from "./parts/artifact-card";
import { ErrorLine } from "./parts/error-line";
import { StatusLine } from "./parts/status-line";

/*
 * One registered renderer per `data-*` part `to-items.ts` produces. assistant-ui hands the part's
 * `{ name, data }` to `render`; the thread renders it through `part.dataRendererUI`.
 */

const StatusDataUI = makeAssistantDataUI<StatusPartData>({
  name: "status",
  render: ({ data }) => <StatusLine data={data} />,
});

const ArtifactDataUI = makeAssistantDataUI<ArtifactPartData>({
  name: "artifact",
  render: ({ data }) => <ArtifactCard data={data} />,
});

const ErrorDataUI = makeAssistantDataUI<ErrorPartData>({
  name: "error",
  render: ({ data }) => <ErrorLine data={data} />,
});

/** Mounted once inside the runtime provider (chat-shell.tsx). */
export function DataUIs() {
  return (
    <>
      <StatusDataUI />
      <ArtifactDataUI />
      <ErrorDataUI />
    </>
  );
}
