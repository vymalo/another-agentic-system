# Thread as Markdown (v1)

- **Status:** contract written 2026-11-14 for the owner's "LibreChat-grade, client-agnostic" plan
  ([ADR 0067](../decisions/0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md), wave 1, slice 6). It describes what the
  web writes today; no other client exists yet, so it is *unverified* by a second client.
- **What it covers:** how one thread becomes one `.md` file, so that a second client (the Flutter app) given the same thread writes the
  **same file**. The input is the document [`exportThread`](chat-api.yaml) answers (`ThreadExport`, the same data as Export JSON) and
  nothing else: no second request, no clock, no stream state. The web's golden files are `web/mock/thread-markdown/*.md`, written from
  threads the mock plays and compared byte for byte by `web/mock/thread-markdown.test.ts` and `web/e2e/export.spec.ts`.
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

## Person

The message as the person wrote it.

## adam

*2 steps, 1 failed.*

The answer as the agent wrote it.

- File: `chart.png` (image/png, 70 bytes)
- Artifact: result — Fix it — <https://github.com/acme/demo/pull/1>
````

Blocks are separated by exactly one blank line, line endings are LF and the file ends with one newline.

## Rules

| Part | A client MUST write |
|---|---|
| **Front matter** | `format`, `version` (1), `title` (the thread's title, whitespace collapsed), `description` (only when the thread has one), `agent` (the thread's `target.agentId`), `thread` (the thread's id), `exported`. Strings are JSON strings (valid YAML), so a title with a quote or a colon is safe. `exported` is the document's `exportedAt` in **UTC, ISO 8601, to the second, ending in `Z`**: never the client's own clock |
| **The thread's id** | is in the file on purpose: it lets the person who exports find the chat again and tell two exports of it apart. It is not a capability (the routes need the owner's sign-in) and the **share link and its nonce are never written**; the JSON holds the id too |
| **Title** | `# <title>` on one line, with `` \ ` * _ [ ] < > & `` backslash-escaped: a title is the person's or a model's text, and must not become a link, a second heading or markup |
| **Turns** | one `##` section per turn, in the order of the thread's own log. `## Person` for what the person did (a `user_message`, a form answer, an action on an interface); `## <name>` for an agent's turn, where `<name>` is the `actor.name` of the turn's first agent event (the thread's `target.agentId` when none says), escaped like the title. An agent turn is everything between two things the person did. A turn with nothing to show has no section |
| **A message** | the `text` of a `user_message` and the **answer** of an agent turn, **verbatim** ([`message-text-v1.md`](message-text-v1.md)): CRLF and CR become LF, blank lines around it are dropped, and it is otherwise not touched. Mentions stay as typed |
| **The answer** | is read as ADR 0031 reads it: for each `messageId` the last `agent_message` is its text; in a turn the answer is the last message with `purpose: answer`, else the last with no `purpose`; words marked `working` and earlier messages are **not** in the file (the web shows them as notes in the panel) |
| **An open code fence** | a message whose fenced code block is never closed would swallow every section after it, so a client MUST write the closing fence after it (same character, same length) |
| **Steps** | **left out**, except one italic line at the top of the turn: `*N steps.*` (`*1 step.*`), with `, M failed` before the full stop when some ended `failed`. N counts the distinct step ids of the turn whose `kind` is not `message`. The label, `detail`, `input` and `output` of a step are never written |
| **Files** | an `artifact` with a `file`: `- File: ` + the file name as a code span (the `filename`, else the artifact's `name`) + ` (<mimeType>, <size> bytes)` with the parts that exist. **No bytes and no link**: the route that serves a file needs the owner's sign-in, so a link in a shared file is dead or wrong |
| **Other artifacts** | `- Artifact: <name>`, then ` — <text>` when the text is one line of at most 200 characters (a branch name, a title; longer text can be a patch or a report), then ` — <https://…>` when the `uri` is an `http` or `https` URL without spaces or `<` `>`. Any other scheme is not written |
| **Interfaces** | a surface of the [UI catalog](ui-catalog-v1.md) is `*[Interface: <types>]*`, see [the placeholder](#the-interface-placeholder), once per surface id (the first turn that shows it), followed, when it holds a `Choices`, by one bullet for each of its questions. The catalog defines no text fallback for a component, so there is none to use |
| **A form answer** | a `ui_action` whose `context.answers` has the [answer shape](ui-catalog-v1.md) is a `## Person` section of one bullet per answer, `- **<question>** <chosen option labels, Other: <text>, typed text>` with `No answer` when empty, then `- **Note** <note>`. Questions and labels are read from the surface's operations earlier in the log (the id and the raw value when it is not there). Any other `ui_action` is `*Used <name> on an interface.*` |
| **A fork** | the log of a fork holds its parent's events up to the fork, then a `thread_forked` event, then its own. The marker is one italic line where the event is: `*The conversation above was copied from the chat "<title>"; what follows was said here.*` (`kind: fork`) or `*From here this is a new version: a message of the chat "<title>" was edited.*` (`kind: edit`), the title escaped and left out when the event has none. The parent's id is not written |
| **Branches** | only the branch the exported thread **is**. An edit or a regenerated answer is a thread of its own whose log does not hold the version it replaced, so the other versions (`‹ 2/3 ›`) are not in the file and nothing marks that they exist |
| **Errors** | `*Error: <message>*` for an `error` event, `*The agent failed: <detail>*` for a `failed` status, `*The turn was stopped.*` for `canceled`, all escaped |
| **A cut log** | when `eventsTruncated` is true, the last line is `*The chat is longer than this file: it holds the first N events.*` |
| **Everything else** | checks, CI, reworks, usage, tools, asks, titles and shares are not written |
| **File name** | `thread-<id>.md`, the id with anything that is not a letter, digit or hyphen dropped, the same pattern as the JSON's `thread-<id>.json`. The media type is `text/markdown;charset=utf-8` |

### The interface placeholder

`[Interface: <types>]`: the component types the surface holds after its operations, in the order each was first defined, one each,
without `Column` and `Stack` (they only lay out) and without `Text`; a surface of layout and text only is `[Interface: Text]`, and one
with no component `[Interface]`. Only letters and digits of a type are kept, because the operations are the agent's. A later definition
of a component replaces the earlier one, `createSurface` starts the surface over and `deleteSurface` removes it. Copy puts the same
line on the clipboard where a surface was (below).

## Not escaped, and why

**Headings and raw HTML in a message are not escaped.** A message is the author's Markdown and is written as the clipboard's Copy writes
it, so what is in the file is what the chat holds. The cost: a message with `## Heading` looks like another turn in an outline, and a
viewer that renders raw HTML would draw a message's `<b>`. The web's own rule is that raw HTML is shown as text
([`message-text-v1.md`](message-text-v1.md)); the file cannot enforce it in another program, so open it in a viewer that sanitises
(every common one does). Only an open code fence is closed, because that one changes the structure of the whole file.

## Limitations (API gaps)

- **A share link has no export.** A reader of a link ([ADR 0040](../decisions/0040-thread-sharing-by-revocable-link.md)) gets the
  AG-UI stream and history, not `ThreadExport`, and the Markdown is defined over the export, so Export Markdown is **not offered on a
  shared link**. A reader-side export would need its own route, with the share's own privacy rules (no step input or output, no owner).
- **The other versions of an edited message are not reachable from the export.** They are in `listThreadBranches`, a second call the
  mapping does not make.
- **No display name of the person.** The log's person is an e-mail address; the file says `Person`, never the address, and cannot say
  who a shared reader is.

## Client recipe for Copy and Quote

Both work on the **source text** of a message, as [`message-text-v1.md`](message-text-v1.md) defines it, never on the drawn page.

| Action | A client |
|---|---|
| **Copy** | puts the message's source text on the clipboard as **`text/plain` only**, never HTML, so no markup is carried into another app. For an agent turn that is its answer (as above) and, where a surface was, a line `[Interface: <types>]` in order. A code block's own Copy copies **only its code**. A clipboard that refuses (permission denied, an insecure page, no clipboard API) is **said in words**, not ignored |
| **Quote** | puts a Markdown block quote into the composer at its caret (the end when it has none), as a paragraph of its own, **without sending**, and puts the focus in the box after the quote. With text selected inside one message it quotes the selection as the page shows it (text, no markup); inside one code block, the code in a fenced block inside the quote (the fence longer than any run of backticks in the code, the language as the label). With nothing selected, or a selection that touches no message or only another one, it quotes the whole message's source text. A selection that touches two or more messages quotes nothing and says why |
| **Read-only** | Quote needs a composer: it is not offered on a shared link or to a person who may only read. Copy is always offered |

A quote is **the person's words**: the API has no reference from a message to the one it quotes, and the text is sent like any other.
