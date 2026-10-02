"use client";

import { createContext, useContext } from "react";
import type { KeptFile } from "@/features/chat/lib/files";

/**
 * The files the thread holds (ADR 0032), for the components of a surface that show one (`Image`):
 * provided by `SurfaceActivity`, from the artifacts of the thread's messages. A surface drawn
 * outside a thread has none, so an `Image` there says it has nothing to show.
 */
const NONE: readonly KeptFile[] = [];
export const ThreadFilesContext = createContext<readonly KeptFile[]>(NONE);
export const useThreadFiles = (): readonly KeptFile[] => useContext(ThreadFilesContext);
