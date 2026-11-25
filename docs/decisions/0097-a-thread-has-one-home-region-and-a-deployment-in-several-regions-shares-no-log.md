# ADR 0097 — A thread has one home region; a deployment in several regions shares no log

- **Status:** **Proposed (2026-11-25).** Fifth gap of the roadmap epic on several regions ([#407](https://github.com/vymalo/another-agentic-system/issues/407)). Nothing is built. Spend and law are the operator's, and this ADR stops where either begins.
- **It builds on:** [ADR 0001](0001-rust-state-machine-on-postgres.md) (one event log per thread, in Postgres), [ADR 0065](0065-a-usage-ledger-per-person.md) (the usage ledger), [ADR 0076](0076-the-orchestrator-as-a-library-in-an-application.md) (a thread's home is fixed) and the database scaling of the chart (CloudNativePG instances, `database.*`, [#290](https://github.com/vymalo/another-agentic-system/issues/290)).

## Context

The orchestrator is stateless over one Postgres, and a thread's log is written by whichever process holds the thread (version check, ADR 0001). That is correct inside one place. A deployment in several regions breaks it two ways. Two regions that can both append to one thread's log would race on the version check across a wide-area link, and a region that is slow or cut off would stall every thread it does not own. And where a person's data lives is a question of latency, of cost and of law, none of which the code can answer for an operator.

So the design has to say what is **per region** (and never leaves), what is **global** (small, rebuildable, and safe to be late), and what **crosses** a region and how.

## Decision

1. **A thread has one home region, fixed at its first message.** The home is a property of the thread row, named in the deployment's configuration by region id, and never changes (the same rule as ADR 0076 for a deployment or an application). Only the home region's orchestrator appends to the thread's log, so the version check of ADR 0001 stays a local, single-writer check. No log is replicated to be written from two places.
2. **The home is chosen by the person's home, not by the thread.** A person has one home region (set when the account is made, the operator's choice by a rule the operator writes: a person's setting, a default, an assignment). Each of their threads takes it. A fork, a shared view and a thread's files stay in the thread's home.
3. **Per region, and never copied elsewhere:**
   - the threads of the people whose home it is: the event log, the search index (ADR 0068), the prompt library (ADR 0070), memory (ADR 0091), push devices, and the secrets of their credentials, sealed (ADR 0046);
   - the artifacts of those threads, in the region's artifact store;
   - the agents that run for those threads, and their stores. An agent is **placed per region**: the same agent card can be served in two regions as two deployments with two stores.
4. **Global, small, and allowed to be late:**
   - the **account directory**: which person has which home region, and nothing else about them;
   - the **agent registry**: which agents exist and in which regions they are placed;
   - the **deployment configuration** (models, limits, roles).

   A global item is rebuildable from the regions and carries no thread content, no message and no secret.
5. **The usage ledger (ADR 0065) is per region.** Each region counts its people's usage in the commit of the event that carries it. A person's month across regions does not exist as one row: a single-region person has none to merge, and a person who moves home is a migration (decision 8), not a merge on every read.
6. **A request goes to the person's home.** The edge routes by the person's home region (a claim of their token or a lookup in the account directory). A region that receives a request for a thread it does not own **does not serve it from a copy**; it answers with a redirect to the home, or, where the operator's edge can do it, forwards. No region reads another's database.
7. **What crosses a region:** a message sent to an agent placed in another region (A2A over the platform's own network path, from the home orchestrator), a model call to a model server in another region where the operator allows it, and the account directory and agent registry of decision 4. **What never crosses:** a thread's log, its files, a person's secret, memory, prompts, search index and ledger. A cross-region call carries the request it needs and never the person's other data.
8. **Moving a person's home is an explicit migration, not a failover.** It is offline for that person's threads (they are read-only while it runs), copies their data in the order log, files, index, ledger, switches the home in the account directory last, and deletes the source after a retention the operator sets. A region going down does not move anyone: its people wait, and the others are not affected.
9. **Backups are per region** (the chart's continuous backups, ADR 0041), restored into the same region. A region's loss is that region's people's outage until restore, which is the price of decision 1; a deployment that cannot accept it runs a database replica inside the region (the `database.instances` of the chart), not a log written from two places.
10. **Where a region's choice is spend or law, it is the operator's.** Which regions exist, which people go to which, whether a model call may leave a region and what a region costs are configuration and policy, written by the operator. The code offers the mechanism (a region id on the thread, the directory, the routing) and no default that decides it. The default is **one region**, and then nothing here is visible.

## Slices

One row per pull request, one repository per row.

| Slice | What it delivers | Repository | Decisive test |
|---|---|---|---|
| R1 | A `region` setting and a home region on the thread row, set at the first message from the person's home, never changed; a write for a thread whose home is not this region is refused | sys | a thread created in region A is refused a write by region B's orchestrator against the same database, and accepted by A (both seen red first) |
| R2 | The account directory (person to home region) as a port with a conformance testkit and an in-memory adapter | sys | the testkit passes on the adapter with a positive count; a person with no entry gets the default region |
| R3 | The edge route: a request for a thread of another region is answered with a redirect to its home, never served | sys | a mock two-region setup: region B answers a read of A's thread with the redirect and reads nothing from A's database |
| R4 | The platform places one agent in two regions, each with its own store, and lists the regions in the registry item | plat | an agent applied with two placements yields two deployments and two stores, and the registry lists both regions |
| R5 | The orchestrator picks, among an agent's placements, the thread's home region | sys | a thread of region A talks to the agent's A placement, a thread of B to B's, on mocks |
| R6 | The home-move migration, as a command that copies in order and switches the directory last | sys | an interrupted move leaves the source whole and the person's home unchanged |

## Consequences

- **The single-writer log is kept**, so the event model, the version check and the transition function do not change. Regions are a routing and placement concern above them.
- **A region's outage is local**, and the price is that its people wait; a replica inside the region is the operator's remedy.
- **A person's usage across regions is not one number.** A client that wants one sums per region, and the ledger does not pretend otherwise.
- **The default stays one region**, so nothing changes for a deployment that does not turn this on.

## Open items

- Whether the account directory is one table replicated to every region or a service each region asks; slice R2 decides by its testkit, and either is behind the port.
- Whether a shared thread's public link is served by its home region only (this ADR says yes) or by an edge cache; a cache would hold content outside the home and needs its own decision.

## Alternatives rejected

- **A thread's log written from two regions with a merge.** It gives up the version check and the pure transition, which are the reason the log is trustworthy, for a latency gain nobody has asked for.
- **A global database.** One region's outage becomes everyone's, and it makes a person's data leave their region by construction.
- **Choosing a region per thread.** A person's data would be in several regions with no one rule for where their secrets, memory and ledger live.
