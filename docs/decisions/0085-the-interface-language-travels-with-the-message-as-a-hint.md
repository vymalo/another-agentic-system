# ADR 0085 — The interface's language travels with the message, as a hint

- **Status:** **proposed 2026-11-22**, at the owner's decisions of that day (another-context
  `plans/2026-11-22-languages.md`, decision rows 14–17). The orchestrator's side is built (the tag and its checks, the card
  read, the activation, `dev/locale-e2e.sh`); no agent lists the extension yet and the web does not send it yet.
  **Amended 2026-11-22, the same day** (the coordinator's ruling): decision 4 — the hint **is logged** on the `user_message`
  and is not kept out of the log, because the log is the state and a hint that changes the agent's words must be replayable;
  the projection still shows nothing of it. The rest of the record stands.
- **Contract:** [`docs/api/locale-v1.md`](../api/locale-v1.md). **Pattern:** [ADR 0008](0008-platform-integration-via-a2a-extension.md).
  **The extension this one is deliberately *not* shaped like:** [ADR 0061](0061-the-person-picks-an-access-tier-for-each-message-the-agent-keeps-to-it.md).

## Context

A person reads the interface in one language and writes to an agent in whatever language they like — and often in a third,
because the agent's own vocabulary is English. Until now the agent saw only the second: the text of the message. It had no way
to know which language the person is *reading*, so an agent that can write Yemba, French, Cameroonian Pidgin and English had to
guess from the words, and guessed wrong whenever the person wrote a short message, a file name or a code snippet.

The owner decided, on 2026-11-22:

- the interface supports **`en-CM`, `fr-CM`, `wes-CM` and `ybb-CM` from day 0** (rows 14–15);
- **the agent's language is the interface's language by default, then the person's** (row 16);
- **the list of languages is the deployment's configuration, not code**, and the wording of the public repositories stays
  neutral — nothing about markets or customers (row 17).

The system already has the mechanism for this: an A2A extension the agent lists, the orchestrator detects on the live card and
activates per message, and a client without it is unaffected (ADR 0008). The access tier (ADR 0061) is the working example: a
URI in `A2A-Extensions` and `message.extensions`, one member in the message's metadata.

## Decision

1. **The interface's language travels with each message, as a hint.** The client sends the **BCP 47 tag** of the interface it
   is drawing, per message: `forwardedProps["vymalo.locale"]` on an AG-UI run, and `{"locale": "<tag>"}` under
   `https://agents.vymalo.com/a2a/extensions/locale/v1` in the A2A message's metadata, with the URI activated in the header and
   in `message.extensions` — the same two places the access tier uses. A message that names none activates nothing and carries
   no member at all.

2. **A tag, not a string, and not a closed list of four.** The core validates the shape of the tag against RFC 5646 (a bounded
   length, ASCII, the grammar's productions, case-insensitive subtags) and **refuses anything else with a 400** rather than
   passing it through. It does **not** hold the four languages of the deployment: which languages an interface offers is the
   deployment's configuration (row 17), so the core accepts any well-formed tag and the configuration names the ones the
   screens offer. A closed enum here would put a market decision in a protocol crate, and ADR 0004's closed enums are about
   protocols, not about data a client sends.

3. **A hint fails open, and that is the whole difference from the tier.** An agent whose card does not list the extension, or
   lists it with a `locale` that is not a tag, is sent the **plain A2A message**: the hint is dropped. It is never a reason to
   send nothing and never a reason to refuse the person's words. The tier (ADR 0061) does the opposite — a tier is a promise
   the person made and is never silently dropped — because a permission that silently widens is a defect, while a language that
   silently falls back to the agent's default is the behaviour every agent has today.

4. **The log records it, and the projection shows nothing of it.** The tag is on the `user_message` (`locale`, omitted when the
   message names none) and on the outbox row that carries it to the send, and it is nowhere else: no AG-UI frame, no activity,
   no badge, no title or description, no search word. **The log is the state**: a hint that changes the agent's words must be
   replayable, so a redelivery of a message that never reached the agent carries the hint its message recorded, a fork's first
   message carries the one its request names, and an edit that names none continues the replaced message's — each as the tier
   does. This is the one place the extension deliberately does not copy the tier: the tier is shown in the interface and a locale
   need not be, so it is recorded without being projected. (An earlier draft of this record kept the hint out of the log
   altogether; it could not answer a redelivery, and a hint the log cannot replay is a hint the agent's words cannot be
   explained by.)

5. **The agent's language: the interface's, then the person's.** The agent answers in the language the hint names when it can; a
   message whose text is in another language is answered in that language, because a person who writes Yemba in a French
   interface is asking for Yemba; and an agent that can write neither answers in its own default and says so in words. The hint
   never changes what the agent may do: it is not a permission, a scope or a tenant.

6. **The agent's own words never fill a high-risk string.** This is a **client rule, not a prompt**, and it holds whatever
   language the hint names. Every string the agent's model writes is *text the person reads*: it is drawn as the person's text
   (Markdown, sanitized), never as interface chrome. The label and the fields of a **trusted component**
   ([ADR 0083](0083-trusted-components-the-agent-names-and-the-client-draws.md)), the wording of a **payment** and the text of
   a **consent** are the client's own reviewed, static strings, translated by people. A model that writes Yemba barely must not
   be able to produce the text of a consent screen, or the label of a button that spends money — and the way to guarantee that
   is to give it no path to that string, not to ask it politely in a prompt.

## Consequences

- An agent that wants the hint lists one URI and reads one member; an agent that does not is unchanged, byte for byte, and so is
  every deployment whose agents list nothing.
- A thread's log says which language each message was written on, so an export explains the agent's answer and a replay sends
  the same message. The interface still shows none of it: a person who reloads in another language sees the thread in the new
  language, and only the messages written afterwards carry the new tag.
- A client must not use the hint to justify drawing anything it did not draw before. The rule of decision 6 is what makes a
  multilingual agent safe to talk to: the agent's language ability widens, the client's own surface does not.
- The four languages of day 0 are configuration, so a fifth is a configuration change and a translated screen — not a protocol
  change, and not a change to this ADR.

## Alternatives considered

- **A setting of the thread** (a column, an event, a control beside the composer): rejected. The interface's language can change
  between two messages (a person switches the screen's language, or writes from another device), and a thread-level value would
  then be a stale preference standing for every message, instead of each message's own tag.
- **`Accept-Language` on the HTTP request only**: rejected. It is a fact about the HTTP call, not about the message; the
  dispatcher sends the message later, from another process, and the header would be lost or invented.
- **A closed enum of the four languages in the core**: rejected by row 17. It would make a deployment's product decision a
  protocol constant, and refuse `de-DE` from a client that has every right to send it.
- **Failing closed, as the tier does**: rejected. A hint that cannot be delivered has a defined fallback (the agent's default),
  and holding a person's message back over the language of their screen would be a worse product for no safety.
- **Projecting it, as the tier is**: rejected. The tier is shown as a badge on the message because the person picked it and
  wants to see it; a locale is ambient, and a badge for it would be interface chrome nobody asked for. Recorded, not shown.
- **Keeping it out of the log, as an earlier draft did**: rejected. A hint the log does not hold cannot be redelivered, cannot
  be continued by a fork and cannot explain an agent's words in an export; the log is the state, and this is state that changed
  what the agent said.

## References

- [`docs/api/locale-v1.md`](../api/locale-v1.md) — the contract: the tag and what is refused, the card, the activation, the
  message, the security rule, the conformance list.
- [ADR 0008](0008-platform-integration-via-a2a-extension.md) — optional extensions, detected from the live card, fail closed on
  what cannot be read.
- [ADR 0061](0061-the-person-picks-an-access-tier-for-each-message-the-agent-keeps-to-it.md) — the shape this one copies, and
  the one rule it inverts.
- [ADR 0083](0083-trusted-components-the-agent-names-and-the-client-draws.md) — the client draws its own interface for what an
  agent asks for; decision 6 is why that is not optional.
- RFC 5646, *Tags for Identifying Languages* — sections 2.1 (`langtag`, `privateuse`) and 4.4.1 (the 35-character bound).
