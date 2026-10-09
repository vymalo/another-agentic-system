---
# The chat agent's remote sub-agent `browser` (adam-rs `a2a:`, docs/reference/agent-files.md "Remote subagents"; ADR 0057). The chart
# renders it into the chat's folder as subagents/browser.md with `browser.chatSubagent`, and replaces the host of the card URL below (the
# dev stack's) with the browser's Service; dev/compose.chat-browser.yaml puts this file's copy (dev/agents/browser/chat-subagent.md) into
# the dev chat's folder. Plain http inside the cluster, which the chat accepts only with A2A_ALLOW_INSECURE_REMOTES=true, set beside it.
# The token is the browser's bearer, from the chat's environment, never from this file. `files: true` (adam-rs ADR 0033): the files of the
# browser's answer (its screenshots and PDFs) become files of the chat's own run, so they reach the person.
description: "A browser: it opens a public web page, reads it and answers with what it says and its URL, and takes a screenshot when you ask it to. Use it when the person names a page, or when an answer must come from a page you can name. It does not sign in or fill in forms that buy, book, post or send."
a2a: http://browser:8080/.well-known/agent-card.json
auth: bearer:BROWSER_A2A_TOKEN
files: true
---
Give it the full URL of the page and say what you want to know from it; it does not see this conversation. Its screenshots come back
as files of yours: the result names each one, and you show it in your answer as the result says.
