# Searching a person's own threads (v1)

- **Status:** contract written 2026-11-15 for the owner's "LibreChat-grade, client-agnostic" plan
  ([ADR 0067](../decisions/0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md), wave 1, slice 7), decided by
  [ADR 0068](../decisions/0068-a-person-searches-their-own-threads.md). It describes what the orchestrator does and what the web's mock
  server does; no client has drawn it yet (the web's box is slice 7b), so it is *unverified* by a client of ours.
- **What it covers:** the one route, [`searchThreads`](chat-api.yaml) (`POST /api/search/threads`), and the rules a server follows so that
  **every server and every client agree on the same threads, the same snippets, the same offsets and the same order**: what a word is,
  how a word is folded, how a query is read, when a document matches, how the snippet is cut and how the hits are ranked. Nothing here
  depends on a database's text search or a ranking function: [`orch_core::search`](../../orchestrator/crates/core/src/search.rs) (Rust) and
  [`web/mock/search.ts`](../../web/mock/search.ts) (TypeScript) are two implementations of this page, and a third reads it the same way.
- **Conformance vectors:** [`examples/thread-search/`](examples/thread-search/): `fold.json` (text to words and keys), `query.json` (a
  query to its terms, or its error), `snippet.json` (a text and terms to the snippet), `corpus.json` with `queries/*.json` (a person's
  threads, and what each request of them answers). The Rust core reads the first three and the mock's tests read all of them; a test of
  each holds this page's numbers to the code.
- **Words:** MUST, SHOULD and MAY are used as in RFC 2119.

A person searches **their own threads and nobody else's** ([ADR 0039](../decisions/0039-nobody-reads-another-persons-thread.md)): the
route needs `thread.read`, which every role has for its own threads, and no role reads more. What it finds is always a subset of what
[`listThreads`](chat-api.yaml) lists for the same `archived` and `branches`. The server enforces this; a client only offers the box.

## Constants

| Name | Value | Meaning |
|---|---|---|
| `MAX_QUERY_LENGTH` | 200 | the longest query, in Unicode scalar values, after it is trimmed |
| `MAX_TERMS` | 8 | the most terms of a query that are used |
| `MIN_TERM` | 2 | a term of fewer code points is dropped |
| `MAX_KEY` | 64 | the longest key in code points: a longer word is not indexed, a longer term is cut |
| `MAX_WORDS` | 3000 | the most words of one document that are searched |
| `MAX_MESSAGE_HITS` | 3 | the most message hits one thread shows |
| `CONTEXT_WORDS` | 6 | the words in front of the first match that a snippet starts with, at most |
| `CONTEXT_UNITS` | 100 | a snippet does not reach further back than this before the first match, in UTF-16 units |
| `WINDOW_UNITS` | 200 | a snippet is about this long, in UTF-16 units |
| `EDGE_UNITS` | 20 | text left over at the very start or end of a document is kept up to this much, in UTF-16 units |

## Words and keys

[Vectors: `fold.json`](examples/thread-search/fold.json)

A **word** is a maximal run of code points of the Unicode general categories L (letter), M (mark) and N (number): `[\p{L}\p{M}\p{N}]+`.
Everything else (space, punctuation, symbols, emoji, `@`, `'`, `-`, `_`, `.`) separates words and is in none. A text has **at most
`MAX_WORDS` words**: the 3,001st and every later one is not a word, so it is searched and marked by nobody. A word has a start and an end,
**UTF-16 code unit offsets** into the text it is in (`end` is the offset after its last unit), so an emoji or a letter outside the Basic
Multilingual Plane counts two.

The **key** of a word is what it is searched by:

1. Normalise it to **NFKD**.
2. Remove every code point of category M.
3. Convert it to lower case (the full Unicode mapping: `toLowerCase` in JavaScript, `to_lowercase` in Rust).
4. Replace `œ` by `oe`, `æ` by `ae`, `ß` by `ss`, and the final sigma `ς` by `σ` (NFKD leaves the first three whole, and French and
   German need them; the sigma has two lower-case forms for one letter).
5. Remove every code point that is not a letter (L) or a number (N): NFKD can leave a symbol behind (the fraction slash of `½`).

A word with **no key** (nothing is left, or the key is longer than `MAX_KEY` code points) is a word, and is never matched. There is **no
stemming, no language and no word segmentation**: a run of Chinese or Japanese letters with no space is one word, so only its start can
match (see the vectors), and the term `walked` does not find the word `walk`; a term is a *prefix* of a key, which covers plurals and typing
as you go.

A **document** is a thread's title, a thread's description, or one message. Its **keys** are those of its words. The 64-code-point limit is
on the key and not the word because NFKD can make a letter into many (`ﷺ` is eighteen).

## What is searched

[Vectors: `queries/`](examples/thread-search/queries/)

| Document | Searched | Never searched |
|---|---|---|
| the title | always | |
| the description | when the thread has one | |
| a person's message | the `text` of every `user_message` | the files they attached |
| an agent's message | the `text` of an `agent_message` that is **final** (`final: true`) and whose `purpose` is not `working` | partial messages (`final: false`), working text, reasoning, steps (their input and output), tool results, surfaces and plans, a form's answers, file names, usage, checks, shares |

A final `agent_message` **replaces** the earlier ones of the same thread with the same `messageId`: only the last is searched (and not at all
when the last is working text). It is the rule that [`thread-markdown-v1.md`](thread-markdown-v1.md) uses to find an agent's answer. A
person's message is never replaced.

**Forks.** A fork's log starts with its parent's events up to the cut ([ADR 0029](../decisions/0029-forking-a-thread-copies-its-log.md)).
**While the parent exists** (the thread's `forkedFrom.threadId` is set) the copied events give **no message hit**: the parent shows them. Once the
parent is deleted the fork stands alone, and its copied events are its own words. A fork's own title, description and events always count.
A thread made by an edit of a message is a **branch**, searched only when the request says `branches: include`, as in `listThreads`.

## The query

[Vectors: `query.json`](examples/thread-search/query.json)

`q` is read like this, in this order:

1. **Trim** the code points U+0020, U+0009, U+000A and U+000D from both ends, and only those (a no-break space is not blank). Nothing left: **400
   `query_empty`**.
2. More than `MAX_QUERY_LENGTH` Unicode scalar values (code points: an emoji is one): **400 `query_too_long`**.
3. Read the words of `q` and take the key of each (no limit on the number of words; there are at most 200 characters).
4. Cut every key to its first `MAX_KEY` code points, drop the keys of fewer than `MIN_TERM` code points, drop a key that an earlier one
   equals, and keep the first `MAX_TERMS`. These are the **terms**.

A query with **no term** (`a`, `!!`) is not an error: **200 with `terms: []` and no result**. Quotes, `OR`, `-`, `*`, `:`, `|`, `&`, `!`,
parentheses and SQL are punctuation or words like any others: **there is no query syntax**. A server MUST NOT put `q` or any text of it into
a SQL string, a query language of a database or a log line; only the terms, which are letters and digits, may reach an index.

## When a document matches

A document **matches** when **every term is a prefix of the key of at least one word of the document** (two terms may be prefixes of the
same word). Terms are never split between documents: `tomatoes basil` finds a message that has both, and not a thread whose one message has
`tomatoes` and whose other has `basil`. A thread is a **hit** when its title, its description or at least one of its messages matches.

## The result

[Vectors: `queries/`](examples/thread-search/queries/)

| Member | |
|---|---|
| `rank` | **2** when the title matches, else **1** when the description matches, else **0** (only messages) |
| order | `rank` descending, then the thread's `id` descending (ids are UUIDv7, so the newest created first). A page that is read later may differ from an earlier one only because a thread was renamed, deleted or written to in between |
| `hits` | the **title** hit, if it matches; the **description** hit, if it matches; then **at most `MAX_MESSAGE_HITS`** message hits, **newest first** (by `seq`, descending) |
| `messageHits` | the **exact** number of matching messages, so a client can say "and 2 more messages" |
| `query.terms` | the terms used, so a client can say what was searched for |

A **message hit** has the message's `seq` (the number to open the thread at: `getThreadHistory?since=<seq>` as for a `#m-<seq>` link), its
`role` (`person` or `agent`) and its `at`. Every hit has a `snippet`.

**Archived threads** are included unless `archived` says otherwise: `include` (the default), `exclude` (not archived) or `only` (archived
only). A hit does not say whether its thread is archived: the `thread` does (`archivedAt`).

## The snippet

[Vectors: `snippet.json`](examples/thread-search/snippet.json)

The snippet of a document's text for the terms is made like this. **No markup is ever written**: a client draws the plain runs and the marked
ones itself and MUST NOT interpret anything of the text as markup.

1. **Flatten.** Replace every run of U+0020, U+0009, U+000A and U+000D by one space, and trim the ends (the whitespace of
   [`thread-markdown-v1.md`](thread-markdown-v1.md)). Everything below is on this text; offsets are UTF-16 code units into it.
2. **The first match** is the first word whose key starts with at least one term. None: no snippet (the document does not match).
3. **Start.** Look at the up to `CONTEXT_WORDS` words in front of the first match; of those that start no further than `CONTEXT_UNITS` before
   the first match starts, take the **earliest**. Its start is the window's start; with none, the match's own.
4. **End.** The window ends at the end of the **last word that ends no later than the start plus `WINDOW_UNITS`**, and never before the end of
   the first match.
5. **Edges.** If the window's first word is the text's first word and starts within `EDGE_UNITS`, the start becomes 0. If its last word is the text's last
   word (there is no later word, not even beyond `MAX_WORDS`) and the text ends within `EDGE_UNITS` after it, the end becomes the length of the text. So `"login" is bad`
   keeps its quotes and `Fix login.` its full stop.
6. **The result:** `text` is the text between start and end, `cutStart` is `start > 0`, `cutEnd` is `end < length`, and `matches` are the words
   inside the window whose key starts with a term, as `{start, end}` **relative to `text`**, in order. A window never starts or ends inside a
   surrogate pair: its edges are word edges, 0 or the length.

A client writes "…" in front when `cutStart` and after when `cutEnd`; it MAY use its own length for what it shows, the offsets always stay
in UTF-16 units of `text`.

## The vectors of the corpus

`corpus.json` holds a person's threads and `queries/*.json` what each request answers: the `terms`, the ordered `results` (`id`, `rank`,
`messageHits`, `hits`) and whether there is a next page, or the `code` of the 400. A request that has `"cursor": "$previous"` sends the `next`
of the answer before it. Each file is one rule at a time: a rank order, the three-hit limit, a replaced message, a fork, an archived thread, a
branch, accents, Cyrillic, CJK, digits, hostile text, no terms, paging and the refusals.

## Limits and errors

| | |
|---|---|
| `limit` | threads per page, 1 to 50, default 20; out of range is 400 `bad_request` |
| `cursor` | the `next` of the page before, **sent back unchanged with the same `q`, `archived` and `branches`**; anything else, another request's cursor included, is 400 `bad_cursor`. What is in a cursor is each server's own |
| `archived`, `branches` | `exclude`, `only`, `include` and `include`; anything else is 400 `bad_request`. `branches` is absent or `include` |
| any other member | 400 `bad_request`. **There is no `owner`**: a person's threads are the only ones there are to search |
| 401, 403 | no sign-in; no `thread.read` |
| 415 | the body is not `application/json` |
| 429 `rate_limited` | a person searches in bursts and then a few times a second; `Retry-After` says how long to wait |
| 503 `search_unavailable` | the store is down or the search took too long; `Retry-After` says when to try again |

The query is **never in a URL** (the route is a `POST`, a safe read: it writes nothing) and a server's log lines hold the number of terms and
nothing of the words.

## Limitations (what this is not)

- **No stemming, no synonyms, no typo tolerance, no phrase or boolean query.** Whole words, by their start.
- **No segmentation of Chinese, Japanese or Thai**: a run of letters with no space is one word.
- **Only the first `MAX_WORDS` words** of a title, a description or a message are searched.
- **The result is the person's own threads**, not shared links ([ADR 0040](../decisions/0040-thread-sharing-by-revocable-link.md)): there is no search
  under `/api/shared` or `/api/public`.
- **File names, step input and output and reasoning are not searched** on purpose (the words of the agent are its final answers).
- **Unicode.** The vectors use only characters of Unicode 13 and earlier; whether another runtime's tables agree beyond that is *unverified*.

## Client recipe

The recipe is in the description of [`searchThreads`](chat-api.yaml) too:

1. If `GET /api/config` has no `search`, offer no box.
2. Debounce about 250 ms, cancel the request in flight when the text changes, and drop an answer that is not for the latest text.
3. Send `{"q": text}` (the server trims, folds and bounds it; do not do it yourself). Draw each result with `thread.title`, and each hit's
   `snippet.text` split at `matches` into plain and marked runs, with "…" where `cutStart` or `cutEnd`. Say `messageHits` beyond the hits shown.
4. Opening a message hit opens the thread at `seq` (`getThreadHistory?since=<seq>`, as for a `#m-<seq>` link); a title or description hit opens
   it as usual.
5. For more, send the same request with `cursor` set to `next`, until `next` is absent.
6. On 429 wait `Retry-After`; on 503 say the search is unavailable and offer to try again.

## API gaps found

None new. A message hit's `seq` closes part of the first gap of [ADR 0067](../decisions/0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md#the-rule):
a message's `seq` is now a field of a search hit; it is still not a field of the other messages of the API.
