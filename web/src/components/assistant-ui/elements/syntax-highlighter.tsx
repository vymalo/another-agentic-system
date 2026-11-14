"use client";

import type { SyntaxHighlighterProps } from "@assistant-ui/react-markdown";
import { type FC, Fragment, useEffect, useState } from "react";
import { highlightCode, languageFor, type Token } from "@/lib/highlight";

/*
 * The code of a fenced block in an answer. It is plain text first and always: the highlighter is a chunk loaded on
 * the first block, so the code is on the screen before any colour, and it stays plain for a language the web does
 * not bundle and when the highlighter fails. A streaming answer rewrites the block many times a second; it is
 * highlighted only once the code has not changed for `QUIET_MS`, which is when the fence closes or the model
 * pauses, so the colours appear once and never flicker; colours of older code are never shown.
 */

const QUIET_MS = 150;

type Highlighted = { code: string; language: string; lines: Token[][] };

export const SyntaxHighlighter: FC<SyntaxHighlighterProps> = ({
  components: { Pre, Code },
  language,
  code,
}) => {
  const [highlighted, setHighlighted] = useState<Highlighted | null>(null);

  useEffect(() => {
    if (!languageFor(language)) return;
    let current = true;
    const timer = setTimeout(() => {
      highlightCode(code, language).then(
        (lines) => {
          if (current && lines) setHighlighted({ code, language, lines });
        },
        () => {
          // plain text stays: a highlighter that fails never fails the answer
        },
      );
    }, QUIET_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [code, language]);

  const lines =
    highlighted && highlighted.code === code && highlighted.language === language
      ? highlighted.lines
      : null;

  return (
    <Pre>
      <Code>
        {lines
          ? lines.map((line, row) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: lines have no identity beyond their position
              <Fragment key={row}>
                {row > 0 ? "\n" : null}
                {line.map((token, column) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: tokens have no identity beyond their position
                  <span key={column} data-token="" style={token.style}>
                    {token.content}
                  </span>
                ))}
              </Fragment>
            ))
          : code}
        {lines && code.endsWith("\n") ? "\n" : null}
      </Code>
    </Pre>
  );
};
