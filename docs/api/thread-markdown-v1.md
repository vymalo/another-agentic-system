# Thread as Markdown (v1)

- **Status:** contract written 2026-11-14 for the owner's "LibreChat-grade, client-agnostic" plan
  ([ADR 0067](../decisions/0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md), wave 1, slice 6), revised the same day after
  two reviews (one of them a second implementation, from this page alone, that wrote the same bytes). It describes what the web writes;
  no other client of ours exists yet, so it is *unverified* by a second client of ours.
- **What it covers:** how one thread becomes one `.md` file, so that a second client (the Flutter app) given the same thread writes the
  **same file**. The input is the document [`exportThread`](chat-api.yaml) answers (`ThreadExport`, the same data as Export JSON) and
  nothing else: no second request, no clock, no stream state.
- **Conformance vectors:** [`examples/thread-markdown/`](examples/thread-markdown/): each `<name>.export.json` is an input and `<name>.md`
  the file a client MUST write from it, byte for byte. Some are threads the mock orchestrator plays, the rest are written for one rule each
  (hostile messages, titles, failures, truncation, forks, artifacts, blank messages and more). `web/mock/thread-markdown.test.ts` holds
  the web to them, and holds this page to the words the code writes.
- **Words:** MUST, SHOULD and MAY are used as in RFC 2119.

The file is for **people**: to read, keep or paste into a note. A program that needs the log uses the JSON, which is the contract for
machines. **Nothing in the Markdown is anything the JSON export of the same person does not hold**, and what the web shows only on demand
(a step's input and output) is left out on purpose.

## Shape

````markdown
---
format: another-agentic-system/thread-markdown
version: 1
title: "Fix the login"
description: "One line, when the thread has one"
agent: "adam"
thread: "0190aaaa-0000-7000-8000-000000000123"
exported: 2026-11-14T19:09:33Z
---

# Fix the login

## *Person*

> The message as the person wrote it,
> every line behind `> `.

## adam

*2 steps, 1 failed.*

> The answer as the agent wrote it.

- File: `chart.png` (image/png, 70 bytes)
- Artifact: result — Fix it — <https://github.com/acme/demo/pull/1>
````

## The one rule that keeps the structure: a message is a block quote

**What a participant wrote is only ever written as a block quote: every line of it behind `> `, an empty line behind a bare `>`, and the
text otherwise untouched.** The file's own lines (the front matter, the `#` title, the `## <who>` headings, the italic lines and the lists
below) are never inside a quote and never start with `>`. In CommonMark a block quote is a container: an open code fence, an open HTML
block or a heading written inside one cannot escape it, so **nothing a person or an agent wrote can become a heading, a turn or the end of
the file**. A client does not repair anything in a message (it closes no fence, escapes no heading): it quotes it.

**Round trip.** A message's text comes back exactly by removing the first two characters, `> `, of every line of its quote; a line that is
exactly `>` is an empty line. Two cases are stated exactly, because a viewer cannot tell them apart and the file can: an *empty* line is
`>` alone, and a line of spaces keeps them (`>` then a space then the spaces). Trailing spaces **inside** a message are kept, so every line
of a quote may end in spaces; only the end of the whole message is trimmed (below).

What counts as the message: CR and CRLF are read as LF; the lines at the start that hold only spaces (U+0020) and tabs (U+0009) are
dropped; the spaces, tabs and line feeds at the end are dropped. That is all: whitespace here is exactly U+0020, U+0009, U+000A and U+000D,
never a language's idea of it, so a no-break space is text. A message with nothing left is written as `*Empty message.*` for the person and
leaves out the agent's answer.

## Rules

| Part | A client MUST write |
|---|---|
| **Front matter** | the members `format`, `version`, `title`, `description`, `agent`, `thread`, `exported`, in that order (`description` only when the thread has one), between lines of `---`. `format` is `another-agentic-system/thread-markdown`, `version` is `1`. `title`, `description`, `agent` and `thread` are JSON strings, which are valid YAML: whitespace runs collapsed to one space and the ends trimmed for the first two, **non-ASCII written as it is**, never `\u` escapes |
| **`exported`** | the document's `exportedAt` in **UTC, ISO 8601, to the second, ending in `Z`**: the fraction is **cut**, never rounded. Never the client's own clock |
| **The thread's id** | is in the file on purpose: it lets the person who exports find the chat again and tell two exports of it apart. It is not a capability (the routes need the owner's sign-in) and the **share link and its nonce are never written**; the JSON holds the id too |
| **Title** | `# <title>` on one line, the title as a short string ([Escaping](#escaping)), `# Untitled chat` when it is empty |
| **Sections** | one `##` section per turn, in the order of the thread's own log. **Every person event opens its own section** (a `user_message`, a form answer, an action on an interface): two messages in a row are two sections. An agent's turn is everything between two person events, or a fork marker, and the end |
| **The person's heading** | is `## *Person*`. An agent's heading is `## <name>` with the name as a short string, and a name's `*` is always escaped (`\*`), so **no agent name can equal the person's heading**, even an agent called `Person` (written `## Person`) or `*Person*` (written `## \*Person\*`). An agent's name is the `actor.name` of the turn's **first** agent event (the thread's `target.agentId` when none has one, `agent` when that is empty too) |
| **A person's message** | the `text` of the `user_message` as a block quote; `*Empty message.*` when it has no text |
| **The answer** | is read as ADR 0031 reads it: for each `messageId` the **last event** is its text, and it keeps the **place of its first event**; in a turn the answer is the last message (by that place) with `purpose: answer`, else the last with no `purpose`. Words marked `working`, earlier messages and messages of blank text are **not** in the file (the web shows them as notes in the panel). It is written as a block quote |
| **Order in an agent's section** | (1) the steps line; (2) the answer; (3) the interfaces, in the order each surface was first shown; (4) **one list** of files and artifacts, in event order; (5) the errors, in event order. Blocks are separated by exactly one blank line. A turn with nothing to write has no section |
| **Steps** | **left out**, except one italic line: `*1 step.*` or `*<N> steps.*`, and `, <M> failed` before the full stop when some ended `failed` (`*2 steps, 1 failed.*`). N counts the distinct step ids of the turn whose `kind` is not `message`; a step's last state decides. The label, `detail`, `input` and `output` of a step are never written |
| **Files** | an `artifact` with a `file`: `- File: ` + the file name as a code span (the `filename`, else the artifact's `name`) + ` (<mimeType>, <size> bytes)` with the parts that exist. **No bytes and no link, even when the artifact also has a `uri`**: the file wins over the `uri`, and the route that serves a file needs the owner's sign-in |
| **Other artifacts** | `- Artifact: <name>`, then ` — <text>` when the text is one line of **at most 200 Unicode scalar values** (code points: `🎉` is one; not UTF-16 units, not bytes; a line break makes it more than one line), then ` — <https://…>` when the `uri` starts with `http://` or `https://` (**either case**) and has no character below U+0021, no U+007F and no `<` or `>`. Any other scheme is not written. Name and text are short strings |
| **Interfaces** | a surface of the [UI catalog](ui-catalog-v1.md) is `*[Interface: <types>]*`, once per surface id, in the turn that first shows it, followed, when it holds a `Choices`, by one list of its questions (`- <question>`, a short string). The catalog defines no text fallback for a component, so there is none to use. `<types>` is [the placeholder](#the-interface-placeholder). A later turn that only updates a surface writes nothing |
| **A form answer** | a `ui_action` whose `context.answers` has the [answer shape](ui-catalog-v1.md) is a person's section of, for each answer, `**<question>**` (a short string) and the answer as a block quote, then `**Note**` and the note as a block quote when there is one. The answer is the chosen option labels, `Other: <text>` and the typed text joined by `, `, or `No answer` when empty; a typed answer keeps its lines, being quoted like a message. Question and labels are read from the surface's operations earlier in the log (the id and the raw value when they are not there). Any other action is `*Used <name> on an interface.*` with the name as a code span |
| **A fork** | the log of a fork holds its parent's events up to the fork, then a `thread_forked` event, then its own. The marker is one italic line where the event is: `*The conversation above was copied from the chat "<title>"; what follows was said here.*` (`kind: fork`) or `*From here this is a new version: a message of the chat "<title>" was edited.*` (`kind: edit`), the title a short string. **With no title** (absent, empty or blank): `*The conversation above was copied from another chat; what follows was said here.*` and `*From here this is a new version: a message of an earlier chat was edited.*`. The parent's id is not written |
| **Branches** | only the branch the exported thread **is**. An edit or a regenerated answer is a thread of its own whose log does not hold the version it replaced, so the other versions (`‹ 2/3 ›`) are not in the file and nothing marks that they exist |
| **Errors** | `*Error: <message>*` for an `error` event, `*The agent failed: <detail>*` for a `failed` status (`*The agent failed.*` without a detail), `*The turn was stopped.*` for `canceled`; the message and the detail are short strings |
| **A cut log** | when `eventsTruncated` is true, the last line is `*The chat is longer than this file: it holds the first <N> events.*`, N the **number of events** of the document (not a sequence number) |
| **Everything else** | checks, CI, reworks, usage, tools, asks, titles and shares are not written |
| **File name** | `thread-<id>.md`, the id with anything that is not a letter, digit or hyphen dropped. The media type is `text/markdown;charset=utf-8` |

The file is LF only, separates blocks with exactly one blank line and ends with one newline.

### Escaping

A **short string** is anything a participant wrote that the exporter writes in its own text: the title, an agent's name, an artifact's
name and text, a question, an error. It is written on one line: runs of space, tab, CR and LF become one space and the ends are trimmed.
Each of `\` `` ` `` `*` `_` `~` `[` `]` `<` `>` `&` is then written behind a backslash. A **word** (between spaces) that a reader would turn into
a link on its own, because it contains `@`, or `www.` after something that is not a letter or a digit, or one of `http:`, `https:`, `ftp:`,
`mailto:` and `xmpp:` (any case), is written as **a code span** instead, with no escapes inside it: GFM reads those links after the
escapes, so an escape does not stop them. A code span uses a delimiter of backticks longer than any run inside the text, with a space inside
when the text starts or ends with a backtick. Nothing else is escaped (so `Fix the bug.` stays), and a message is never escaped: it is quoted.

### The interface placeholder

`[Interface: <types>]`: the component types the surface holds after its operations, in the order each was first defined, one each,
**separated by `, `**, without `Column` and `Stack` (they only lay out) and without `Text`; a surface of layout and text only lists its
non-layout types, so `[Interface: Text]`, and a surface of nothing but layout lists its layout types, in the same order (`[Interface: Column]`,
`[Interface: Column, Stack]`); with no component at all it is `[Interface]`. A surface **deleted in the same turn** has no component: `[Interface]`. Only
letters and digits of a type are kept, because the operations are the agent's. A later definition of a component replaces the earlier one,
`createSurface` starts the surface over and `deleteSurface` removes it. Copy puts the same line on the clipboard where a surface was
(below).

## What is not escaped, and why

**A message is written verbatim, inside its quote.** It is the author's Markdown and the file keeps it, headings, links and raw HTML
included. The quote makes it harmless to the *file*: it cannot end a turn, forge one or swallow the rest. It does not make raw HTML inert
inside the quote: a program that renders raw HTML will render a message's `<b>`. The web's own rule is that raw HTML is shown as text
([`message-text-v1.md`](message-text-v1.md)); the file cannot enforce it in another program. The file is meant to be read as text or in a
viewer that does not render raw HTML; that the common Markdown viewers do not (or do) is *unverified* here, nobody checked them.

## Limitations (API gaps)

- **A share link has no export.** A reader of a link ([ADR 0040](../decisions/0040-thread-sharing-by-revocable-link.md)) gets the
  AG-UI stream and history, not `ThreadExport`, and the Markdown is defined over the export, so Export Markdown is **not offered on a
  shared link**. A reader-side export would need its own route, with the share's own privacy rules (no step input or output, no owner).
- **The other versions of an edited message are not reachable from the export.** They are in `listThreadBranches`, a second call the
  mapping does not make.
- **No display name of the person.** The log's person is an e-mail address; the file says `*Person*`, never the address, and cannot say
  who a shared reader is.

## Client recipe for Copy and Quote

Both work on the **source text** of a message, as [`message-text-v1.md`](message-text-v1.md) defines it, never on the drawn page, except
for a selection (below).

| Action | A client |
|---|---|
| **Copy** | puts the message's source text on the clipboard as **`text/plain` only**, never HTML, so no markup is carried into another app. For an agent turn that is its answer (as above) and, where a surface was, a line `[Interface: <types>]` in order. A code block's own Copy copies **only its code**. A clipboard that refuses (permission denied, an insecure page, no clipboard API) is **said in words**, visibly and to a screen reader, not ignored |
| **Quote** | puts a Markdown block quote into the composer at its caret (the end when it has none), as a paragraph of its own, **without sending**, and puts the focus in the box after the quote. If the caret is inside a word the quote goes after that word, so a word is never split; a second quote is added after the first, which stays. With text selected inside one message it quotes the selection as the page shows it (text, no markup, and never the label or the Copy button of a code block's header); inside one code block, the code in a fenced block inside the quote (the fence longer than any run of backticks in the code, the language as the label). **A selection that touches rendered math** is not quoted as drawn (a formula's drawn text holds its hidden MathML too, so it would be written twice): the source of the smallest run of whole top-level blocks that covers it is quoted, and the whole message when that cannot be told. With nothing selected, or a selection that touches no message or only another one, it quotes the whole message's source text. A selection that touches two or more messages quotes nothing and says why; the note goes away at the next selection or action |
| **A bound** | the composer has no limit of its own, so Quote keeps its own: **20,000 characters** (UTF-16 units) of the text it quotes. A longer text is cut at the last whole line that fits, and the note says "The quote was cut at 20,000 characters." |
| **Read-only** | Quote needs a composer: it is not offered on a shared link or to a person who may only read. Copy is always offered |

A quote is **the person's words**: the API has no reference from a message to the one it quotes, and the text is sent like any other.
