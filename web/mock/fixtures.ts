import type { components } from "../src/lib/api/schema";

type Agent = components["schemas"]["Agent"];

export const AGENTS: readonly Agent[] = [
  {
    id: "coder",
    name: "Coder",
    description: "Implements a change and opens a pull request.",
    cardUrl: "http://coder.agents.svc/.well-known/agent-card.json",
    releases: {
      defaultChannel: "production",
      channels: { production: "coder-r47", staging: "coder-r51" },
      revisions: ["coder-r53", "coder-r51", "coder-r47"],
    },
  },
  {
    id: "reviewer",
    name: "Reviewer",
    description: "Reviews a pull request and reports findings.",
    cardUrl: "http://reviewer.agents.svc/.well-known/agent-card.json",
  },
];

export const DEV_USER = "dev@example.com";
