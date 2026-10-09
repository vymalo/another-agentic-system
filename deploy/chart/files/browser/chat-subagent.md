---
# The chat agent's remote sub-agent `browser` (adam-rs `a2a:`, docs/reference/agent-files.md "Remote subagents"), rendered into the chat's
# folder as subagents/browser.md only with `browser.chatSubagent` (off: adam-agent at the pinned revision refuses a remote sub-agent at a
# plain http URL that is not this machine, and the browser's Service is plain http; ADR 0057). The chart replaces the host of the card
# URL below with the browser's Service. The token is the browser's bearer, from the chat's environment, never from this file.
description: "A browser: it opens a public web page, reads it and answers with what it says and its URL, and takes a screenshot when you ask it to. Use it when the person names a page, or when an answer must come from a page you can name. It does not sign in or fill in forms that buy, book, post or send."
a2a: http://browser:8080/.well-known/agent-card.json
auth: bearer:BROWSER_A2A_TOKEN
---
Give it the full URL of the page and say what you want to know from it; it does not see this conversation.
