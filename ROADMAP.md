# Roadmap to launch

The front door for all four repositories of the another-agentic family: where the project is going, what is left,
and where you can help. *Checked 2026-10-18.*

## What this is

Four repositories make one product. Each name below is short for the repository in the same row.

| Short name | Repository | Role |
|---|---|---|
| **sys** | [another-agentic-system](https://github.com/vymalo/another-agentic-system) | The orchestrator (Rust, one Postgres event log) and the chat surface (Next.js). It also holds this roadmap and the epics. |
| **adam** | [another-adam-rs](https://github.com/vymalo/another-adam-rs) | The durable agent framework, and the coder: an agent that takes a coding task to a verified pull request. |
| **plat** | [another-agentic-platform](https://github.com/vymalo/another-agentic-platform) | The Kubernetes operator and the Platform API that run agents as services. |
| **img** | [another-agentic-images](https://github.com/vymalo/another-agentic-images) | The toolchain container images that agents work in. |

The goal is a coder that an operator can offer to many people at once. In these repositories that needs four things,
and each is a milestone below: isolation between people (M1), a record and a limit of what each person uses (M2), a
platform that holds the load (M4), and data protection and trust (M5).

Payments, pricing and the plan for any particular market are the business of whoever runs a service on these building
blocks. They are not open source, and they left this roadmap on 2026-11-15: M3 keeps its number and nothing else.

## Where we are, honestly

- **The MVP is complete against its build order, and proven on mocks only.** Scripted models, WireMock agents, a mock
  GitHub, a mock sign-in. No live model, GitHub.com or live web page was used in any proof. Complete is not
  accepted: the owner, who judged the first build not ready on 2026-10-01, has not tried this state. See
  [`docs/vision.md`](docs/vision.md) and [`docs/mvp.md`](docs/mvp.md).
- **The first isolation step is merged:** [adam#117](https://github.com/vymalo/another-adam-rs/pull/117) makes a run pod
  see only its own run (adam ADR 0041). Its known limits are the first tickets of M1.
- **Nothing has been measured live.** No load test, no cost per person.
- **There is no launch date.** The order below is the order of dependencies, not a schedule.

## Three quality goals, alongside the milestones

On 2026-11-14 the owner set three goals that run beside M1 to M5 and are not launch milestones: a coder as good as
OpenCode and above, this system "LibreChat-grade" with every feature usable by a second client (a Flutter app is
planned), and the platform at a "kagent-grade minimum". Each has a plan of its own, and the plan is where its slices
and their state live. *Checked 2026-11-18.* The share is the slices merged to `main` against the slices of the plan:
a count, not a measure of effort. As everywhere in this roadmap, all of it is proven on tests and mocks, none on a
live model or a real cluster.

| Goal | Plan | Merged | Share | Left |
|---|---|---|---|---|
| **adam**: a coder as good as OpenCode, and above | [adam ADR 0044](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0044-paged-tools-a-stable-prompt-prefix-a-plan-skills-commands-and-project-context.md), 17 slices | The loop survives bad model output (1), paged reads (2), command output kept whole and read back by page (3), OpenCode removed (5), a prompt prefix that holds across turns and a cache key (4), a plan every agent has (6), a repository's own instructions shown to the coder (7), skills on by default with a repository's own skills (8), the repeated-call rule, time limits and a cancel that reaches children (10), compaction (9), more kinds of question and forms read back (11), commands served by the agent and announced by an A2A extension (12), references a run reads and never writes (13), MCP timeouts, server instructions and prompts as commands (14), commands that keep running (16), images in, off by default (15a), diagnostics, a definition and references from a language server (17, [adam#169](https://github.com/vymalo/another-adam-rs/pull/169)) | 17 of 17, 100 % | The plan is complete. Deferred by the owner, and not counted: images made inline by a model (15b). Next for this goal: a faster coder ([sys#345](https://github.com/vymalo/another-agentic-system/issues/345)), starting with tool calls that run at the same time ([adam ADR 0049](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0049-the-tool-calls-of-a-turn-run-at-the-same-time.md), part A merged in [adam#170](https://github.com/vymalo/another-adam-rs/pull/170)) |
| **adam**: a subagent in the background, as a session | [adam ADR 0045](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0045-a-subagent-can-run-in-the-background-as-a-session.md), 8 slices | The runtime (1), the five `subagent_*` tools (2), remote sessions (5); two of the three parts of the `child-sessions/v1` extension (6a, 6b) | 3 of 8, 38 % | In progress ([sys#332](https://github.com/vymalo/another-agentic-system/issues/332)): reports during a turn (3), limits (4), the extension's last part (6c), the coder's own sessions (7), asking from a child (8). This system's side, child threads: the contract ([ADR 0072](docs/decisions/0072-child-sessions-are-nested-threads-followed-through-child-sessions-v1.md)) and the core are merged; the store, the agent side and the web follow |
| **sys**: LibreChat-grade, client-agnostic | [ADR 0067](docs/decisions/0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md), 35 slices | The usage page, highlighted code and math, regenerate, "My tool servers", UI catalog version 7 (more kinds of question), a theme, quote, copy and export as Markdown, questions that show what they ask about and the agent's plan (UI catalog version 8); search, the API and the box in the web ([ADR 0068](docs/decisions/0068-a-person-searches-their-own-threads.md)); the page "My prompts" (P1: list, create, edit, delete, copy a shared prompt); the `/` menu of prompts and the form that fills one (P2); and, not yet counted, the server half of sharing with roles (P4, [ADR 0070](docs/decisions/0070-a-persons-prompt-library.md)) | 12 of 35, 34 %, and the server half of one more (P4): a slice counts when a person can use it in the web | Read aloud and speech; artifact versions, the panel and live previews; the prompt library's sharing in the web (P4, in review) and P3's `/` commands; MCP prompts (P5); memory; import; `/` commands; attachments; tags; temporary chats; French. The eight API gaps a second client meets are listed in ADR 0067 |
| **plat**: a kagent-grade minimum | [plat §59c](https://github.com/vymalo/another-agentic-platform/blob/main/docs/architecture/10-control-plane-and-crds.md#59c-operator-increments-kagent-grade-minimum), 16 increments | The plan (K0), the CRDs read what adam-rs's former operator accepted (K1), status on shared objects (K2), metrics (K4), an OpenAPI document and an agent's events (K8), `aapctl validate` and `render` (K9); K9b, `aapctl` over the API, with a pasted token and with its browser sign-in; traces of the operator and the Platform API (K5); and tool discovery, an MCP server's reachability and tools in a `ToolProvider`'s status (K3) | 9 of 16, 56 % | In progress ([sys#344](https://github.com/vymalo/another-agentic-system/issues/344), the owner's decisions AD-052 to AD-056): several namespaces (K6: the chart's half merged, the operator's next), then two replicas (K7), a tool catalogue (K10), a bring-your-own image (K11), run pods and agents under gVisor (K14). Wait for adam: approvals per tool (K12). On hold with no cluster: the first real agent (K13) |

## Tracks, one epic each

Beside the milestones, the work is split into tracks so that several people or agents can work at once without
editing the same files. Each track is one epic in sys: its slices, each one pull request; its source of truth; and a
section *Working on this epic without colliding* that names the paths it owns. *Filed 2026-11-18.*

| Track | Repo | Epic | Note |
|---|---|---|---|
| A subagent in the background, as a session | adam, sys | [sys#332](https://github.com/vymalo/another-agentic-system/issues/332) | adam ADR 0045, slices 3 to 8, then the chat's side |
| Hooks around the agent loop | adam | [sys#333](https://github.com/vymalo/another-agentic-system/issues/333) | Starts after tool calls at the same time, part C |
| An agent asks the person for a credential | adam, sys | [sys#334](https://github.com/vymalo/another-agentic-system/issues/334) | Waits for the owner's choices; after hooks |
| LibreChat-grade, what is left of ADR 0067 | sys | [sys#335](https://github.com/vymalo/another-agentic-system/issues/335) | |
| The API a second client needs | sys | [sys#336](https://github.com/vymalo/another-agentic-system/issues/336) | The eight gaps of ADR 0067 |
| Memory, then its refinement | sys | [sys#337](https://github.com/vymalo/another-agentic-system/issues/337) | |
| What used what, in the app | sys | [sys#338](https://github.com/vymalo/another-agentic-system/issues/338) | Builds on M2 ([sys#274](https://github.com/vymalo/another-agentic-system/issues/274)) |
| Windows: browser, web and app previews | sys, adam | [sys#339](https://github.com/vymalo/another-agentic-system/issues/339) | |
| Diagrams as a UI catalog component | sys | [sys#340](https://github.com/vymalo/another-agentic-system/issues/340) | |
| Workspaces that last | sys, adam | [sys#341](https://github.com/vymalo/another-agentic-system/issues/341) | Step 1: a conversation keeps its repository checkouts |
| Missions, then federation | sys | [sys#342](https://github.com/vymalo/another-agentic-system/issues/342) | |
| Agent-team: several primary agents in one chat | sys | [sys#377](https://github.com/vymalo/another-agentic-system/issues/377) | **R1, the first release** (not deferred to R2's hooks: the approval gate is the orchestrator's). [ADR 0074](docs/decisions/0074-agent-team.md), accepted 2026-11-19; distinct from missions |
| Search by meaning and relations | sys | [sys#343](https://github.com/vymalo/another-agentic-system/issues/343) | ADR 0071 |
| The rest of the kagent-grade minimum | plat | [sys#344](https://github.com/vymalo/another-agentic-system/issues/344) | AD-052 to AD-056 |
| A faster coder | adam | [sys#345](https://github.com/vymalo/another-agentic-system/issues/345) | Worked by one session already: do not start it elsewhere |

Until part C of tool calls at the same time merges in adam ([ADR 0049](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0049-the-tool-calls-of-a-turn-run-at-the-same-time.md)),
its loop, the coder's tools and `bench/` are being changed: the epics say which paths to leave alone until then.

## How the work is organised

Milestones, then epics, then tickets.

- A **milestone** is a step to launch: M1, M2, M4 and M5 (M3, payments, is not in these repositories). Its label is `launch-m1` to `launch-m5`.
- An **epic** is the parent issue of a milestone. All five are in sys, and each holds the live list of its tickets.
- A **ticket** is one piece of work: a GitHub issue in the repository that owns the code, and a sub-issue of its epic.
  Size is S, M or L, a rough guide to the effort.
- A **decision** is an ADR: `docs/decisions/` in [sys](docs/decisions) and in
  [adam](https://github.com/vymalo/another-adam-rs/tree/main/docs/decisions), and `AD-NNN` entries in
  [plat](https://github.com/vymalo/another-agentic-platform/blob/main/docs/architecture/11-decisions.md). img keeps
  its rules in its README and CLAUDE.md.
- An **open question** lives in [`docs/open-questions.md`](docs/open-questions.md) until an experiment or an ADR
  closes it.

| Label | Meaning |
|---|---|
| `launch-m1` to `launch-m5` | The milestone the issue belongs to. |
| `epic` | The parent issue of a milestone (sys only). |
| `ticket` | One piece of work, owned by the repository it is in. |
| `roadmap` | Part of this roadmap. |
| `good first issue` | Small, self-contained, and explained in the issue. A good way in. |
| `help wanted` | The maintainers would like outside hands on it. |
| `owner-decision` | The owner decides something before it is built, or while it is built. Comments are welcome. |
| `security` | Hardening or abuse control. These are already-public work items, not vulnerability reports: report a vulnerability privately ([SECURITY.md](SECURITY.md)). |

## How the milestones depend on each other

M1 comes before M2. M4 and M5 run alongside and can start now. Launch needs all four. M3, payments, is an operator's
own and is not in these repositories.

```mermaid
flowchart LR
  subgraph chain["In order"]
    M1["M1<br/>A coder safe for many people"] --> M2["M2<br/>Usage and limits"]
  end
  subgraph alongside["Alongside"]
    M4["M4<br/>Scale and operations"]
    M5["M5<br/>Data protection and trust"]
  end
  M2 --> L(["Launch"])
  M4 --> L
  M5 --> L
```

## M1: A coder that is safe for many people

**Goal.** In production, one person's run cannot read, change or act on another person's work, or reach the coder's
secrets. The coder knows who the person is and checks their GitHub access before it touches a repository.

**Done when**

- Commands run only in run pods that hold no secret and see one run, and production is configured that way.
- No command that git can be told to run (filters, hooks, embedded repositories) runs where a secret is readable.
- The orchestrator sends a signed assertion of the person, the person links their GitHub account, and the coder
  checks read and push access before it adds, clones, fetches or pushes a repository, failing closed.
- Each person's git mirror and package cache are apart from everyone else's.
- Run pods reach the network only through a proxy with an allow-list.
- Where the cluster offers a sandboxed runtime, run pods start under gVisor, and a run pod outside it is refused.

**Epic:** [M1: A coder that is safe for many people](https://github.com/vymalo/another-agentic-system/issues/273).
Related: [sys#159](https://github.com/vymalo/another-agentic-system/issues/159) (the token's `sub` as the user key).
Not in this milestone: limits (M2). gVisor, which the owner had left out on 2026-10-17, is back in
it by the owner's decision of 2026-11-15: the last ticket below.

**Done already.** [adam#117](https://github.com/vymalo/another-adam-rs/pull/117), merged 2026-10-18: a run pod mounts
only its own run. Still true today: run pods are off by default (`runPods.enabled`), so the live deployment runs
commands in the coder's own container, next to the GitHub App key file.

| Ticket | Repo | Size | Labels | Who and when |
|---|---|---|---|---|
| [Run the coder's own git apart from its secrets](https://github.com/vymalo/another-adam-rs/issues/118) | adam | L | security, help wanted | Anyone with Linux and git depth, after agreeing the approach in the issue |
| [Build ADR 0038: no model key in run pods](https://github.com/vymalo/another-adam-rs/issues/119) | adam | L | security, owner-decision | Needs the owner's decision first |
| [Production runs commands only in run pods](https://github.com/vymalo/another-adam-rs/issues/120) | adam | M | security | Anyone |
| [Discuss and agree ADR 0064: the person reaches the agent and links their GitHub](https://github.com/vymalo/another-agentic-system/issues/278) | sys | M | owner-decision, help wanted | Everyone may comment, the owner decides |
| [B1b: the person assertion, GitHub linking and the token lease](https://github.com/vymalo/another-agentic-system/issues/279) | sys | L | | Anyone, after the ADR 0064 ticket |
| [B1c: the coder verifies the person and checks their access to each repository](https://github.com/vymalo/another-adam-rs/issues/121) | adam | L | | Anyone, after the ADR 0064 ticket |
| [B1d: bump adam and prove the whole chain on mocks](https://github.com/vymalo/another-agentic-system/issues/280) | sys | M | | Anyone, after B1b and B1c |
| [A git mirror per person](https://github.com/vymalo/another-adam-rs/issues/122) | adam | M | | Anyone, after B1c |
| [The package cache: per person, or read-only in run pods](https://github.com/vymalo/another-adam-rs/issues/123) | adam | M | owner-decision | Needs the owner's decision first |
| [Run-pod egress: deny by default, allow through a proxy](https://github.com/vymalo/another-adam-rs/issues/124) | adam | L | security, help wanted | Anyone with Kubernetes networking experience |
| [Close the slot-directory race in the coder](https://github.com/vymalo/another-adam-rs/issues/125) | adam | M | security, help wanted | Anyone with Linux filesystem and Rust experience |
| [D15: run pods through the operator](https://github.com/vymalo/another-agentic-platform/issues/36) | plat | M | | Anyone, after "Production runs commands only in run pods" |
| [K14: run pods under gVisor, a sandboxed runtime class](https://github.com/vymalo/another-agentic-platform/issues/54) | plat | L | security, owner-decision | Needs the owner's decision first (plat P-040); then anyone with Kubernetes and container-runtime experience |

## M2: Usage and limits per person

**Goal.** Know what each person uses, in model tokens and sandbox time, and refuse a run before it starts when the
person is over their limit or has too many runs at once.

**Done when**

- Each person's model tokens (priced by model) and sandbox seconds are recorded, and the person can read them.
- A run is refused before dispatch, with a reason the person can act on.
- One person cannot fill the dispatcher.
- The edge limits requests, and the orchestrator can tell clients apart behind the ingress.

**Epic:** [M2: Usage and limits per person](https://github.com/vymalo/another-agentic-system/issues/274).
Related: [sys#170](https://github.com/vymalo/another-agentic-system/issues/170) (token budgets).

**Done already** (2026-10-28):

- **A usage ledger per person.** It is written in the commit of each usage event, keeps a task's totals with the call
  reported twice counted once, and outlives an erased thread ([ADR 0065](docs/decisions/0065-a-usage-ledger-per-person.md),
  proposed).
  - `GET /api/me/usage?month=YYYY-MM` answers the caller's own month, priced by the `usage.prices` table of the
    configuration. Prices are in integers, and a model with no price is said so, never counted as free.
  - A retention sweep removes old months (`usage.retentionMonths`, 24 by default).
  - Proven by `dev/usage-ledger-e2e.sh` on the mocks. [sys#281](https://github.com/vymalo/another-agentic-system/issues/281)
    is closed.
- **Sandbox time per run** is reported by the coder and kept in the ledger (`sandbox` of `usage/v1`,
  [adam#126](https://github.com/vymalo/another-adam-rs/issues/126) closed).
- **The dispatcher gives each person a fair share of its slots.** The person with the fewest jobs running gets the
  next free slot, across replicas ([ADR 0066](docs/decisions/0066-a-fair-share-of-the-dispatcher.md), proposed).
  `dispatcher.order: fifo` brings the old order back. [sys#283](https://github.com/vymalo/another-agentic-system/issues/283)
  is closed.
- **The chart can limit each client address at the Ingress** (`ingress.rateLimit`, a Traefik `RateLimit` middleware).
  - It is proven on kind (`ratelimit-e2e.yml`, run 40695070844, 2026-10-28).
  - It is **off by default**: whether the netcup ingress shows each client's real address is still
    [open question 51](docs/open-questions.md), and turning the limit on waits for that check.
  - [sys#284](https://github.com/vymalo/another-agentic-system/issues/284) is closed.
- **Left:** refusing a run before dispatch ([sys#282](https://github.com/vymalo/another-agentic-system/issues/282)), which
  waits on the owner's decision on the limits. Nothing of M2 has run on the live deployment.

| Ticket | Repo | Size | Labels | Who and when |
|---|---|---|---|---|
| [A usage record per person, priced by model](https://github.com/vymalo/another-agentic-system/issues/281) | sys | L | | Anyone. Design first: the pull request starts with an ADR |
| [Report sandbox time per run as usage](https://github.com/vymalo/another-adam-rs/issues/126) | adam | M | | Anyone |
| [Check limits before a run starts](https://github.com/vymalo/another-agentic-system/issues/282) | sys | L | owner-decision | After the usage record and the owner's decision on the limits |
| [A fair share of the dispatcher for each person](https://github.com/vymalo/another-agentic-system/issues/283) | sys | M | help wanted | Anyone |
| [A rate limit at the edge, and the client address behind the ingress](https://github.com/vymalo/another-agentic-system/issues/284) | sys | M | security, help wanted | Anyone. The live check of the ingress needs access to a deployment |

## M3: Payments (not in these repositories)

Payments, plans and pricing belong to the operator of a service, not to the open-source building blocks. This
milestone left the roadmap on 2026-11-15 and keeps its number so that M4 and M5 keep theirs. What stays here is what
an operator's payment system needs from these repositories, and that is M2: a usage record per person
([ADR 0065](docs/decisions/0065-a-usage-ledger-per-person.md)) and a check of a person's limit before a run starts
([sys#282](https://github.com/vymalo/another-agentic-system/issues/282)).

## M4: Scale and operations

**Goal.** The databases, the dispatcher and the run pods hold about 200 runs at once at peak, and someone is told
before they run out.

**Done when**

- The orchestrator's and the coder's databases run three instances with resources, and the orchestrator's has a
  pooler.
- The coder prunes finished runs.
- A load test on the mocks reports the limits and the first bottleneck.
- Capacity metrics exist, and the chart ships alert rules as examples.

**Epic:** [M4: Scale and operations](https://github.com/vymalo/another-agentic-system/issues/276).
Related: [sys#151](https://github.com/vymalo/another-agentic-system/issues/151) (backups),
[sys#149](https://github.com/vymalo/another-agentic-system/issues/149) (live verification of the deployment),
[sys#157](https://github.com/vymalo/another-agentic-system/issues/157) (retention).

**Done already.** Continuous backups of the orchestrator's database to S3 are built and passed a kind end-to-end in CI.
They were not run on the live deployment, and [sys#151](https://github.com/vymalo/another-agentic-system/issues/151)
is still open. The orchestrator can use a pooler (`database.directUrl`, [sys#309](https://github.com/vymalo/another-agentic-system/pull/309)),
and the chart can run its database as three instances with resources and put a CloudNativePG pooler in front of it
([sys#290](https://github.com/vymalo/another-agentic-system/issues/290), `database.instances`, `database.resources` and
`database.pooler`, all off by default). That is proven on renders and by a kind end-to-end that CI had not run when this was
written; it has not run on the live deployment. The coder's database has not changed: one instance with no pooler. Nothing
is pruned, and nothing has been load-tested.

| Ticket | Repo | Size | Labels | Who and when |
|---|---|---|---|---|
| [CNPG for scale: three instances, resources and a pooler](https://github.com/vymalo/another-agentic-system/issues/290) | sys | M | help wanted | Anyone. CloudNativePG experience helps |
| [The coder's database for scale, and pruning finished runs](https://github.com/vymalo/another-adam-rs/issues/127) | adam | M | help wanted | Anyone. The default retention needs the owner's agreement |
| [Load test on the mocks: 200 runs at once](https://github.com/vymalo/another-agentic-system/issues/291) | sys | L | help wanted | Anyone |
| [Capacity metrics and alerts](https://github.com/vymalo/another-agentic-system/issues/292) | sys | M | help wanted | Anyone. Prometheus experience helps |

## M5: Data protection and trust

**Goal.** Let people delete their data, and keep a deployment that is open to many people from being abused. Which
law applies, and what it asks of an operator, depends on where the service runs: legal advice, terms of service and
a privacy policy are the operator's and are not in these repositories.

**Done when**

- A person can delete their account and everything of theirs: threads, their own MCP servers and sealed secrets,
  their GitHub link and files. Usage records are kept only as long as the operator configures.
- There are abuse controls: a sign-up gate, detection of CPU mining on run pods, and a kill switch per person.

**Epic:** [M5: Data protection and trust](https://github.com/vymalo/another-agentic-system/issues/277).
Related: [sys#145](https://github.com/vymalo/another-agentic-system/issues/145) (personal data, GDPR),
[sys#158](https://github.com/vymalo/another-agentic-system/issues/158) (a privacy note for testers).

**Done already.** Deleting one thread erases it, with its files and its link ([ADR 0043](docs/decisions/0043-deleting-a-thread-erases-it.md)).
Nothing deletes a person.

| Ticket | Repo | Size | Labels | Who and when |
|---|---|---|---|---|
| [Delete my account](https://github.com/vymalo/another-agentic-system/issues/294) | sys | M | | Anyone |
| [Abuse controls](https://github.com/vymalo/another-agentic-system/issues/295) | sys | M | security, help wanted | Anyone. The sign-up gate needs the owner's choice between the options |

## Good first issues

Small, outside any epic, and explained in the issue. Each also carries `help wanted`.

| Issue | Repo | Size | What it takes |
|---|---|---|---|
| [The coder reads a repository's package.json without following links](https://github.com/vymalo/another-adam-rs/issues/128) | adam | S | Rust. Use `safe_fs` and add a test |
| [Move GitHub Actions off Node 20](https://github.com/vymalo/another-adam-rs/issues/129) | adam | S | Update the `actions/*` majors |
| [Move GitHub Actions off Node 20](https://github.com/vymalo/another-agentic-system/issues/297) | sys | S | The same, in this repository |
| [Move GitHub Actions off Node 20](https://github.com/vymalo/another-agentic-platform/issues/37) | plat | S | The same, in this repository |
| [Move GitHub Actions off Node 20](https://github.com/vymalo/another-agentic-images/issues/13) | img | S | The same, in this repository |

## Decisions waiting on the owner

Anyone may comment on these in the issue or the pull request. The owner decides.

**Tickets labelled `owner-decision`**

| Ticket | Milestone | What is to decide |
|---|---|---|
| [adam#119](https://github.com/vymalo/another-adam-rs/issues/119) | M1 | Build ADR 0038: no model key in run pods |
| [sys#278](https://github.com/vymalo/another-agentic-system/issues/278) | M1 | Agree ADR 0064: the person reaches the agent and links their GitHub |
| [adam#123](https://github.com/vymalo/another-adam-rs/issues/123) | M1 | The package cache: per person, or read-only in run pods |
| [plat#54](https://github.com/vymalo/another-agentic-platform/issues/54) | M1 | gVisor for run pods: run pods only at first, required when set, BuildKit and Podman refused with it until proven (plat P-040) |
| [sys#282](https://github.com/vymalo/another-agentic-system/issues/282) | M2 | The limits that the check enforces |

**Proposed decisions.** Their ADRs say *proposed*: the owner has not confirmed them. Status as written in each ADR on
2026-10-18.

| Decision | Where it stands |
|---|---|
| [sys ADR 0064](docs/decisions/0064-the-person-reaches-the-agent-and-links-their-github.md): the person reaches the agent and links their GitHub | Proposed 2026-10-18. Nothing is built. Fifteen decisions for the owner to confirm, and open questions 85 to 89 |
| [adam ADR 0041](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0041-a-run-pod-sees-only-its-own-run.md#decisions-for-the-owner-to-confirm): a run pod sees only its own run | Proposed, and built in [adam#117](https://github.com/vymalo/another-adam-rs/pull/117). Eight decisions for the owner to confirm, and [known limits](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0041-a-run-pod-sees-only-its-own-run.md#known-limits-not-built) that M1 closes |
| [plat §60a](https://github.com/vymalo/another-agentic-platform/blob/main/docs/architecture/10-control-plane-and-crds.md#60a-admin-dashboard-v0): the admin dashboard v0 | Each slice lists its decisions "for the owner to confirm" in the section. They are not copied here |
| [sys ADR 0037](docs/decisions/0037-the-mvp-is-complete-against-its-build-order.md): the MVP is complete against its build order | Proposed until the owner has tried the stack and agrees |
| [sys ADR 0042](docs/decisions/0042-the-thread-list-is-the-owners.md): the thread list is the owner's | Proposed. Built on mocks, with drag and drop outstanding |
| [sys ADR 0043](docs/decisions/0043-deleting-a-thread-erases-it.md): deleting a thread erases it | Proposed. Built on mocks, backend and web |
| [sys ADR 0047](docs/decisions/0047-one-ui-for-web-desktop-and-mobile-each-signs-in-as-a-public-oauth-client.md): one UI for web, desktop and mobile | The direction is accepted. The rest is proposed. Token storage on the desktop is the owner's to confirm |
| [sys ADR 0059](docs/decisions/0059-a-thread-opens-at-its-end-and-older-turns-load-on-scroll-up.md): a thread opens at its end | Proposed and built. The owner has not tried it |
| [sys ADR 0060](docs/decisions/0060-the-client-keeps-a-bounded-copy-of-recent-threads-behind-a-chatstore-port.md): a bounded copy of recent threads on the client | Proposed. Design only |
| [sys ADR 0061](docs/decisions/0061-the-person-picks-an-access-tier-for-each-message-the-agent-keeps-to-it.md): the access tier | Proposed. The orchestrator and the web are built against mocks |
| [sys ADR 0062](docs/decisions/0062-the-person-picks-a-network-tier-for-each-message-the-agent-keeps-to-it.md): the network tier | Proposed. Nothing is built |
| [sys ADR 0063](docs/decisions/0063-an-openai-compatible-model-endpoint-per-thread-on-the-orchestrator.md): a model endpoint per thread | Proposed. Nothing is built |

## How to help

1. **Pick an issue.** Start with a [good first issue](#good-first-issues) or a `help wanted` ticket of a milestone.
   The "Who and when" column says what an issue needs first.
2. **Claim it** by commenting on the issue. One person per ticket at a time. If you stop, say so, so someone else can
   take it.
3. **Agree the approach first** on tickets that ask for it: the large ones, the security ones, and any marked
   "design first". Write the approach in the issue before you write code.
4. **Follow the repository's contributing guide:**
   [sys](CONTRIBUTING.md),
   [adam](https://github.com/vymalo/another-adam-rs/blob/main/CONTRIBUTING.md),
   [plat](https://github.com/vymalo/another-agentic-platform/blob/main/CONTRIBUTING.md),
   [img](https://github.com/vymalo/another-agentic-images/blob/main/CONTRIBUTING.md).
   Open a pull request against `main`. A maintainer merges it when CI is green. AI-assisted work is welcome and is
   declared in the pull request.
5. **Report a security problem privately,** never in a public issue: [SECURITY.md](SECURITY.md).

## Keeping this current

The epics hold the live ticket lists, so this page does not repeat what they track. Change `ROADMAP.md` when a
milestone, an epic or the order changes. A pull request that finishes a milestone updates this file in the same pull
request.
