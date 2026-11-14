"use client";

import { useEffect, useRef, useState } from "react";

export type UseCopyToClipboardOptions = {
  copiedDuration?: number;
};

/** What a person is told when the browser would not put the text on the clipboard. */
export const COPY_FAILED = "Could not copy: the browser did not allow it.";

/**
 * Puts text on the clipboard as `text/plain` (`navigator.clipboard.writeText`: never HTML). `isCopied` is true for a while after
 * a success, and `failed` for a while after a refusal (permission denied, an insecure page, no clipboard API): a button says
 * which in words, it never ignores the refusal.
 */
export const useCopyToClipboard = ({ copiedDuration = 3000 }: UseCopyToClipboardOptions = {}) => {
  const [isCopied, setIsCopied] = useState<boolean>(false);
  const [failed, setFailed] = useState<boolean>(false);
  const copiedTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const scopeGenerationRef = useRef(0);

  useEffect(
    () => () => {
      scopeGenerationRef.current += 1;
      if (copiedTimerRef.current === undefined) return;

      clearTimeout(copiedTimerRef.current);
      copiedTimerRef.current = undefined;
      setIsCopied(false);
      setFailed(false);
    },
    [],
  );

  const show = (ok: boolean) => {
    if (copiedTimerRef.current !== undefined) clearTimeout(copiedTimerRef.current);
    setIsCopied(ok);
    setFailed(!ok);
    copiedTimerRef.current = setTimeout(() => {
      copiedTimerRef.current = undefined;
      setIsCopied(false);
      setFailed(false);
    }, copiedDuration);
  };

  const copyToClipboard = (value: string) => {
    if (!value) return;
    const scopeGeneration = scopeGenerationRef.current;
    let written: Promise<void>;
    try {
      written =
        typeof navigator === "undefined" || !navigator.clipboard
          ? Promise.reject(new Error("no clipboard"))
          : navigator.clipboard.writeText(value);
    } catch (error) {
      written = Promise.reject(error);
    }
    written.then(
      () => {
        if (scopeGeneration === scopeGenerationRef.current) show(true);
      },
      () => {
        if (scopeGeneration === scopeGenerationRef.current) show(false);
      },
    );
  };

  return { isCopied, failed, copyToClipboard };
};
