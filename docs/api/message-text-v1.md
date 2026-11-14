# Message text (v1)

- **Status:** contract written 2026-11-14 at the owner's request ("make the UI agnostic, same for the APIs: a Flutter app is
  coming"). It describes what the web draws today; no other client exists yet, so it is *unverified* by a second client.
- **What it covers:** the text of an agent's answer (the `text` of an `agent_message`, and the live text of
  [`text-stream-v1.md`](text-stream-v1.md)) as a client draws it. Surfaces of the UI catalog are
  [`ui-catalog-v1.md`](ui-catalog-v1.md); this page is about plain text with Markdown in it.
- **Words:** MUST, SHOULD and MAY are used as in RFC 2119. A client that follows the MUSTs draws any answer safely; the SHOULDs are
  how the web draws it.

The text is **untrusted**: an agent wrote it from pages and files it read. Everything below makes a client draw it without running
or fetching anything it did not mean to.

## Dialect

A client MUST read the text as **CommonMark with the GitHub Flavored Markdown extensions** (tables, strikethrough, task lists,
autolinks), and as nothing else: no footnotes, no front matter, no directives.

## Rules

| Part of the text | A client |
|---|---|
| **Raw HTML** (`<script>`, `<img onerror=...>`, any tag) | MUST NOT render it. It is shown as the text that was written |
| **Links** | MUST open only `http` and `https` URLs, outside the app (a new tab, the system browser), and MUST NOT pass the app's own credentials, cookies or referrer to them. Any other scheme (`javascript:`, `data:`, `file:`...) is not a link: show its text |
| **Images** `![alt](url)` | MUST NOT fetch the URL by itself: a URL can carry what the agent read. Show the alt text (or the word "image") and, for an `http(s)` URL, a link to it. Files an agent hands over are not Markdown images: they come as artifacts and `Image` components, which a client fetches by their reference |
| **Fenced code** (` ```lang ` ... ` ``` `) | MUST show the code exactly as written, in a monospace block that scrolls sideways, with the language tag, if any, as its label. A client MAY colour it if it can. **A language it does not know, and a fence with no tag, MUST be shown as plain text, never as an error**. Inline code is never coloured |
| **Display math** | see below |
| **Single dollar** (`$5 or $10`) | is **never** math: it is a price |
| **`\(...\)` and `\[...\]`** | are **not** math: they are text, as written |

## Display math

Math is **display math only**, written between `$$` lines. A client that can draw math (TeX syntax) MUST read it by exactly this
rule, so the same answer is a formula in every client:

1. A block that starts with a line holding only `$$` and ends with a later line holding only `$$` is a **display formula**; the
   lines between are its TeX source.
2. A paragraph that is `$$...$$` and nothing else (a short formula on one line) is a display formula too.
3. `$$` **anywhere else is text, exactly as written**: "In bash, `$$` is the pid; print $$ twice." and "pay $5 $$ or $10 $$ tomorrow"
   contain no formula.
4. A `$$` block that is **never closed** is text. (A streaming answer has not reached its end; a half-written formula is not
   shown as an error.)
5. `$$` inside inline code or a fenced block is code.

A formula the client cannot draw (a TeX error, a command it does not support) MUST be shown as its source, marked as an error
(a colour AND something that is not colour), never as a thrown error and never as nothing. A client MUST NOT let a formula do
more than draw: no links, no images, no ids or styles set from the TeX (`\href`, `\url`, `\includegraphics`, `\htmlId`,
`\htmlClass`, `\htmlStyle`, `\htmlData` are refused), and a formula whose macros expand without end MUST be cut and shown as an
error. A client that cannot draw math shows the whole block as text, with its `$$` lines.

For assistive technology a client SHOULD expose the formula itself (MathML or the platform's equivalent), not its TeX source.
A formula wider than the screen SHOULD scroll sideways and be reachable by keyboard.

## Limits

A client draws text of any size; for these it SHOULD stop doing more than showing plain text, because highlighting and typesetting
run on the thread that draws the screen:

| What | SHOULD show plain text above | Why |
|---|---|---|
| A formula | **5,000 characters** (as a block of code) | typesetting 50,000 characters took 1.9 s and made 4 MB of tree in the web |
| A fenced code block | **20,000 characters or 500 lines** | colouring 200,000 characters took 1.3 s in the web |

These are the web's numbers; another client MAY choose its own, and SHOULD say which.

## Streaming

While the text is still arriving, a client SHOULD NOT colour code (the block grows on every piece and the colours would flicker:
coloured, plain, coloured) and MUST follow rule 4 for `$$`. It colours the block once, when the text is final.

## How a client is built from this

The web's implementation, its libraries, its tests and its limits are in
[`web/README.md`](../../web/README.md#code-and-math-in-answers); that page is *how the web does it*, this one is what any client
does.
