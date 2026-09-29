import { MarkdownTextPrimitive } from "@assistant-ui/react-markdown";
import type { ReactNode } from "react";
import remarkGfm from "remark-gfm";

const plugins = [remarkGfm];

function ExternalLink({ href, children }: { href?: string | undefined; children?: ReactNode }) {
  return (
    <a href={href} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  );
}

const components = { a: ExternalLink };

/** Markdown for message text. Raw HTML stays disabled (react-markdown default). */
export function MarkdownText() {
  return (
    <MarkdownTextPrimitive
      className="md"
      remarkPlugins={plugins}
      components={components}
      smooth={false}
    />
  );
}
