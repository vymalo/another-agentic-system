---
name: write-adr
description: Record, amend or supersede an architecture decision (ADR) in another-agentic-system (docs/decisions/), and check it against the repo's invariants (protocol-only, optional extensions, stateless, pure core). Use whenever a design choice is made or changed.
---

# Writing an ADR

## Procedure

1. **Check the invariants first** (CLAUDE.md → *Invariants*). A decision that
   adds a hard dependency on an agent host, gateway or SDK contradicts ADR 0007
   — either make it an optional, capability-detected protocol extension
   (ADR 0008 pattern) or supersede 0007 explicitly.
2. **Next number:** `ls docs/decisions` → `NNNN` = highest + 1, zero-padded.
3. **File:** `docs/decisions/NNNN-kebab-case-title.md`:

   ```markdown
   # ADR NNNN — Title

   - **Status:** accepted (YYYY-MM-DD)

   ## Context
   What forces the decision; link the evidence.

   ## Decision
   What we do, stated so it can be checked.

   ## Consequences
   What becomes easier, harder, or required elsewhere.
   ```

   Add *Alternatives rejected* when there were real ones.
4. **Register it** in `README.md` → *Decisions* table, and link it from the doc
   section it affects (architecture, orchestrator, …).
5. **Changing an existing decision:**
   - small correction → amend in place and add
     `Amended (YYYY-MM-DD): …` to its Status line;
   - reversal → new ADR, and set the old one's status to
     `superseded by ADR NNNN`.
   Never silently rewrite history.
6. **Open questions** it closes: move them to *Closed* in
   `docs/open-questions.md` with the ADR link.
7. **Processes** it introduces get a Mermaid pair (`sequenceDiagram` +
   `stateDiagram-v2`).

## Verify before opening the PR

```sh
npm --prefix tools/docs-check ci
node tools/docs-check/check-docs.mjs   # must print "docs OK"
```
