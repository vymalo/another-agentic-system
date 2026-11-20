"use client";

import { useMessagePartText } from "@assistant-ui/react";
import type { SyntaxHighlighterProps } from "@assistant-ui/react-markdown";
import { type FC, Fragment, useEffect, useState } from "react";
import { highlightCode, languageFor, type Token, withinLimits } from "@/lib/highlight";

/*
 * The code of a fenced block in an answer. It is plain text first and always: the highlighter is a chunk loaded on
 * the first block, so the code is on the screen before any colour, and it stays plain for a language the web does
 * not bundle, for a block over the size limit (`withinLimits`) and when the highlighter fails.
 *
 * It is coloured only when the answer is final. A streaming answer rewrites the block many times a second, and
 * colouring each version would flicker (coloured, plain on the next piece, coloured again): so while the text part
 * is running the block is plain, and when it ends the block is coloured once and stays so.
 */

type Highlighted = { code: string; language: string; lines: Token[][] };

export const SyntaxHighlighter: FC<SyntaxHighlighterProps> = ({
  components: { Pre, Code },
  language,
  code,
}) => {
  const running = useMessagePartText().status.type === "running";
  const eligible = !running && languageFor(language) !== null && withinLimits(code);
  const [highlighted, setHighlighted] = useState<Highlighted | null>(null);

  useEffect(() => {
    if (!eligible) return;
    let current = true;
    highlightCode(code, language).then(
      (lines) => {
        if (current && lines) setHighlighted({ code, language, lines });
      },
      () => {
        // plain text stays: a highlighter that fails never fails the answer
      },
    );
    return () => {
      current = false;
    };
  }, [eligible, code, language]);

  const lines =
    eligible && highlighted && highlighted.code === code && highlighted.language === language
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
