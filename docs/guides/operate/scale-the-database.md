---
sidebarTitle: Scale the database
---

# Scale the database, and pool its connections

This page gives the deployment's one Postgres cluster more instances, so that it survives the loss of a node, and puts a
connection pooler in front of it when more than one orchestrator process shares the database. It is the chart's
`database.instances`, `database.resources` and `database.pooler.*`. It is not the reference: every value, its default and what
the chart refuses are in the [chart's README](../../../deploy/chart/README.md#scaling-the-database-issue-290).

**How far this page is proven (2026-11-28).** Everything here was read from `deploy/chart/templates/cnpg-pooler.yaml`,
`deploy/chart/values.yaml`, `deploy/chart/templates/_validate.tpl`, the chart's README and
[the orchestrator's side](../../api/config.md#database-behind-a-pooler). **No cluster command and no `helm template` was run
for this page:** there was no cluster, and Helm 4.3.2, the version the chart needs, was not on the machine that wrote it. The
kind end-to-end `pooler-e2e` passed on `main` on 2026-11-27 (run 45838381839, job 139973196435, at `e501f2c`, as the
repository's status note records), though the scenario's own
[README](../../../deploy/chart/tests/pooler-e2e/README.md) still says it had not run when it was written. **Unverified:** the
production cluster, a failover, a real load, and the resource numbers, which are a starting point and not a measurement.

Every name with `my-` in it is a placeholder, except the objects the chart names itself: it fixes their prefix to
`another-agentic` (`fullnameOverride`), so the app Secret is `another-agentic-db-app`, the pooler `another-agentic-db-pooler`,
the cluster's read-write Service `another-agentic-db-rw` and the orchestrator `another-agentic-orchestrator`, whatever the
release is called.

```mermaid
sequenceDiagram
  accTitle: Interaction: scaling the database and adding a pooler
  accDescr: A sequence diagram, between Operator, CloudNativePG, the primary and its replicas, the Pooler (PgBouncer) and the Orchestrator.
  actor Op as Operator
  participant K as CloudNativePG
  participant PG as primary and replicas
  participant P as Pooler (PgBouncer)
  participant O as Orchestrator
  Op->>K: database.instances: 3
  K->>PG: two replicas and two volumes, the primary keeps serving
  Op->>K: database.pooler.enabled: true
  K->>P: two PgBouncer pods, transaction, max_prepared_statements 200
  O->>P: database.url, for the request pool, the dispatcher and the NOTIFYs
  P->>PG: server connections as the role orchestrator
  O->>PG: database.directUrl, for the migrations and then LISTEN
```

```mermaid
stateDiagram-v2
  accTitle: Lifecycle: from one database pod to a scaled, pooled cluster
  [*] --> Single: the default render, one instance, no pooler
  Single --> Scaled: database.instances raised, the cluster settles
  Scaled --> Pooled: database.pooler.enabled, the Pooler and the new ConfigMap in one sync
  Pooled --> Waiting: the orchestrator starts before the Pooler's Service has an endpoint
  Waiting --> Pooled: the pooler answers and the pod becomes ready
  Pooled --> Scaled: the value off again, the orchestrator reads uri as before
  Scaled --> Single: instances back to 1
  Single --> [*]
```

## 1. When to scale, and which of the three levers

Two things are worth scaling for, and they are different: **more orchestrator processes** (each holds a pool, and Postgres
has a fixed number of connections), and **a database that survives the loss of a node**. A third, `database.resources`, is
about one pod's size, not about either.

| Value | What it does |
|---|---|
| `database.instances` | the Cluster's `spec.instances`. `3` is one primary and two replicas streaming from it, each on a volume of `storage.size`; CloudNativePG promotes a replica when the primary is lost |
| `database.resources` | the Cluster's `spec.resources` for each PostgreSQL pod. Empty: nothing is rendered |
| `database.pooler.enabled` | renders a CloudNativePG `Pooler` of type `rw` in front of the primary, and points the orchestrator's `database.url` at it |

**On one node the replicas are copies, not protection.** CloudNativePG prefers, but does not require, a node of its own for
each instance (`enablePodAntiAffinity`, `preferred`). Two replicas on the same node survive a lost pod and not a lost node.

## 2. What the pooler is for

With many replicas of the orchestrator, each holds a pool, and every pool costs the database a server connection. A
PgBouncer in transaction mode between them lets every replica keep a small pool while the database sees few connections.

::: in-depth Why the pool mode is not a value
**The pool mode is not a value.** The chart writes `transaction`, and nothing else: the orchestrator's connections are made
for it (`SET LOCAL`, transaction-scoped advisory locks). Session mode would need no pooler at all, and statement mode breaks
transactions.
:::

::: in-depth Why max_prepared_statements is above zero
**`max_prepared_statements` must be above 0**, which needs PgBouncer 1.21 or later. The orchestrator's driver names its
prepared statements (`tokio-postgres` names them `s0`, `s1`, … in each process, so two replicas use the same names), and the
setting is what lets PgBouncer keep them across server connections. At 0 the first statements of two clients collide on one
server connection. The collision was observed with sqlx's names (`prepared statement "sqlx_s_1" already exists`, PgBouncer
1.22.0, [`docs/api/config.md`](../../api/config.md#database-behind-a-pooler)); that `tokio-postgres`' names collide the same
way (`s0`) is *unverified*. The chart writes `200`, and the render is refused at 0, with
the key absent, or with a value that is not a whole number above 0. `pool_mode` among the parameters is refused too.
:::

::: in-depth The two sizes that decide the connections
**The two sizes that decide how much reaches Postgres.** `default_pool_size` is the server connections one pooler pod opens
per user and database; `max_client_conn` is the client connections one pod accepts. The orchestrator's most is about
`pooler.instances` × `default_pool_size` server connections, and with the migrations', the listeners', the agents' and the
operator's own it must stay well below the database's `max_connections` (PostgreSQL's default is 100, and CloudNativePG does
not set one). Two pods × 20 is 40.
:::

## 3. What must never go through the pooler

The orchestrator takes **two** URLs, and the split is not a preference — it is what a transaction-mode pooler cannot carry
([why, and what was observed](../../api/config.md#database-behind-a-pooler)):

| Connection | URL | Why it cannot be pooled |
|---|---|---|
| The migrations at boot | `database.directUrl` | the migrator holds a **session** advisory lock across its statements; a pooler hands the server connection to someone else between two transactions |
| The `LISTEN` connection of the wakeup, and the in-process agents' notifier | `database.directUrl` | `LISTEN` is session state; PgBouncer's own feature map lists it as never supported in transaction pooling |
| `orchestrator usage backfill`, `orchestrator secrets rewrap` | `database.directUrl` | a command holds a session across statements and has no use for a pooler |
| The request pool, the dispatcher, the inbox, the purge and sweep workers, the in-process agents' journal, the `NOTIFY`s | `database.url` | each is a statement or a transaction |

**The agents' own databases are never pooled.** The chat agent's database and each coder's keep their URIs on the cluster's
`-rw` Service, whether the pooler is on or off; nothing here looked at whether adam's run store could use a pooler. No
NetworkPolicy changes either: the chart's policies do not restrict the databases, so the orchestrator reaches the pooler and
the pooler reaches the primary.

## 4. The two URLs, and why the Deployment can build them

::: in-depth Why the Deployment builds both URLs
The orchestrator takes a URL as one secret, and a pooler is another host. So with the pooler on, the Deployment does not
mount the app Secret's `uri` key (which names the cluster's own host). It builds both URLs from that Secret's `username`,
`password` and `dbname`:

```yaml
- { name: ORCH_DB_USER,     valueFrom: { secretKeyRef: { name: another-agentic-db-app, key: username } } }
- { name: ORCH_DB_PASSWORD, valueFrom: { secretKeyRef: { name: another-agentic-db-app, key: password } } }
- { name: ORCH_DB_NAME,     valueFrom: { secretKeyRef: { name: another-agentic-db-app, key: dbname } } }
- name: ORCH_DB_POOLED_URL      # database.url:       { env: ORCH_DB_POOLED_URL }
  value: "postgresql://$(ORCH_DB_USER):$(ORCH_DB_PASSWORD)@another-agentic-db-pooler.my-namespace.svc:5432/$(ORCH_DB_NAME)"
- name: ORCH_DB_DIRECT_URL      # database.directUrl: { env: ORCH_DB_DIRECT_URL }
  value: "postgresql://$(ORCH_DB_USER):$(ORCH_DB_PASSWORD)@another-agentic-db-rw.my-namespace.svc:5432/$(ORCH_DB_NAME)"
```

Three properties make that safe, and each is in the chart's README:

- **No secret is in a ConfigMap or in the Deployment.** The ConfigMap holds two `{ env: … }` references; the Deployment
  holds `$(…)` references, which the kubelet fills in when it starts the container, from the variables declared above them
  — including those from a `secretKeyRef`, so the three Secret keys come first. A reference to an undefined variable stays
  as the text `$(NAME)` and would fail to log in.
- **The password needs no escaping.** CloudNativePG generates it with 64 characters from `a-z`, `A-Z` and `0-9` only, which
  a URL carries as they are. A password you put in the app Secret yourself (the chart does not offer it) must be as plain,
  or the URL breaks, and the render cannot see it.
- **There is no second credential.** The pooler authenticates the orchestrator's role by the role's own password, through
  the `auth_query` CloudNativePG sets up for it.

The pooler's Service is named like the `Pooler`, `<fullname>-db-pooler` (a Pooler may not carry the name of its Cluster).
:::

## 5. What it costs

::: in-depth What the starting resources come to
With `instances: 3` and the pooler on with two pods, the chart's starting resources come to **five pods where the default
render has one**, **1.7 CPU and 3.25Gi requested** (3 × 500m + 2 × 100m, 3 × 1Gi + 2 × 128Mi), a memory limit of 6.5Gi, and
**two more volumes of `storage.size`** — 30Gi in all with the default 10Gi. The CloudNativePG operator is not the chart's.

Those are the numbers of a starting point. A database that holds the chat of a few people is not known to need them, and a
smaller one is a value. The pooler's own defaults are a starting point too, not a measurement.
:::

## 6. Turn it on

Order matters, because the orchestrator image has to understand the key before the value that uses it appears.

1. **`orchestrator.image.tag`** at or after the commit that introduced `database.directUrl`. An older image refuses the key
   and exits 78. CI reads the pooler render through the pinned image once it is, and says so until then; `pooler-e2e`
   refuses to start before.
2. **The CloudNativePG operator must know the `Pooler` kind.** It is older than the `Database` kind, so any release this
   chart already needs has it (*unverified* below 1.30.1, which is the end-to-end's version and the vendored schema's), and
   its default PgBouncer image must be 1.21 or later (1.25.2 in CloudNativePG 1.30.1). A pinned `spec.pgbouncer.image` is
   not a value here: the operator's own choice is the one CloudNativePG tests.
3. **`database.instances: 3` first**, if you want replicas, and let the cluster settle (`kubectl get cluster`).
   CloudNativePG adds two instances and two volumes and does not restart the primary (*unverified* on a real cluster).
4. **Then `database.pooler.enabled: true`.** In one sync the `Pooler`, the new ConfigMap and the Deployment arrive together;
   the orchestrator may start before the pooler's Service has an endpoint, fail to connect, and restart until it does.

Turning it off again is the same change the other way: the orchestrator reads `uri` as before, and the `Pooler` goes.

A value the chart refuses fails the render with a message that names the key. The whole numbers are checked whether or not
the pooler is on, so a wrong one is found when it is written and not the day it is turned on.

## 7. Check that it worked

***Unverified*: no cluster.** What to expect comes from the chart's templates and from the scenario's own assertions.

```sh [terminal]
kubectl -n my-namespace get cluster,pods
kubectl -n my-namespace get pooler
kubectl -n my-namespace logs deployment/another-agentic-orchestrator | grep -i 'directUrl'
```

You should see the cluster `Ready` with three instances, a `Pooler` with two pods, and — in the orchestrator's log — the
line that says `database.directUrl is set`, which the scenario asserts at boot. Then read the ConfigMap: its `url` and
`directUrl` are `{ env: … }` references, not URLs, and no rendered object carries a password.

::: in-depth What the scenario checks, session by session
What the scenario looks at, once the orchestrator has served a request, is who connected: every session from the
orchestrator pod's own address is the wakeup's `LISTEN`, the sessions from the two pooler pods' addresses are the requests
and none is a `LISTEN`, and no session of a pooler pod ran the migration lock, `_sqlx_migrations` or DDL. The pooler's own
metrics are the other half: transactions and parsed prepared statements for the database `orchestrator` rose, and its pool
mode for it is transaction.
:::

## 8. What CI proves, and what it does not

`pooler-e2e` applies the chart's own objects to a throwaway kind cluster: the CloudNativePG operator, three instances with
the starting resources, a pooler of two pods, then the orchestrator image the chart pins, with the chart's configuration, a
mock issuer and a mock agent. Two lines of that configuration differ from the chart's — `server.environment` and the issuer,
because a production orchestrator takes an https issuer only and a kind cluster has no certificate authority for one — and
the script asserts that those two lines are the whole difference. It runs when the database or orchestrator templates, the
helpers, the validation, `values.yaml`, the mock issuer, the mock agent or the scenario change, weekly, and on demand
([the workflow](../../../.github/workflows/pooler-e2e.yml)).

**What it does not prove:** the production cluster, a failover (the primary is never killed), a load, a count of connections
under one, the agents' own databases (that deployment has none), the production check of the issuer, PgBouncer versions
other than the operator's default, and the migration as more than evidence when the sampler misses it — a migration of an
empty database is a fraction of a second.

## Next

What each connection may do through a pooler, with what was observed against PgBouncer 1.22.0:
[the orchestrator's side](../../api/config.md#database-behind-a-pooler). The cluster these instances belong to, and the
agents' databases beside the orchestrator's: [One database cluster](../../../deploy/chart/README.md#one-database-cluster).
Backups are a separate value and off by default: [Backups](../../../deploy/chart/README.md#backups). The limit at the edge
is a separate page: [Rate limit](rate-limit.md).
