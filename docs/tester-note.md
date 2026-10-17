# What the system keeps, and for how long (tester note)

**DRAFT for the owner.** Written on 2026-10-17 with the chart's database backups (issue #151, [ADR 0041](decisions/0041-deployed-with-helm-on-kubernetes-secrets-by-externalsecret.md)
amended that day). Nothing in it has been sent to anyone, no tester has seen it, and the owner has not read it: the wording, and the numbers behind it, are the owner's to change. It is the
statement that [ADR 0040](decisions/0040-thread-sharing-by-revocable-link.md) ("the retention is stated to testers") and [ADR 0043](decisions/0043-deleting-a-thread-erases-it.md) (decision 7:
backups keep a deleted thread until they expire) say the invitation to testers makes. Facts are marked *verified* (with date and source) or *unverified*.

## For the invitation (a sentence to adapt)

> Your conversations are stored in a database that only the people who run the system can reach, and only you can read yours in the app (unless you share a link). If you delete a conversation it is erased from
> the app at once. The database is also backed up, and a backup keeps what the database held for up to about 30 days: a conversation you deleted can still be in a backup until it expires. The
> backups are used for nothing but restoring the system, by the person who runs it. If you want a conversation erased sooner, ask them; they can erase it from the live database, not from a backup.

## The facts behind it

| Question | Answer | Status |
|---|---|---|
| Who reads a person's threads in the app? | Only their owner, an administrator included, unless the owner shares a link ([ADR 0039](decisions/0039-nobody-reads-another-persons-thread.md), [ADR 0040](decisions/0040-thread-sharing-by-revocable-link.md)). | built, proven on mocks; not tried on a cluster |
| What does deleting a thread do? | Erases the thread, its events and its files from the live system, and ends its share link, at once ([ADR 0043](decisions/0043-deleting-a-thread-erases-it.md)). | built, proven on mocks |
| Are there backups? | **Only when the owner turns them on** (`backup.enabled` in the chart, off by default). Until then there are none, and the live database is the only copy. | by the chart's render |
| What do they hold? | The whole database: every thread's events (text, step inputs and outputs), the share links, the people's own tool-server definitions. WAL is archived as it is written and a base backup is taken every day, to an S3 bucket. | the chart's design; not run |
| For how long? | A recovery window of **28 days** (the default; the chart refuses more than 30 and the unit `m`). The newest base backup older than the window is kept too, so the oldest data is up to one backup interval (a day) older than the window: a thread deleted today is gone from the backups after **about 29 days**, and within 30 if the plugin deletes an obsolete backup only "after the next backup completes". | the rule is *verified 2026-10-17* ([retention.md](https://raw.githubusercontent.com/cloudnative-pg/plugin-barman-cloud/main/web/docs/retention.md), [objectstore_types.go](https://raw.githubusercontent.com/cloudnative-pg/plugin-barman-cloud/main/api/v1/objectstore_types.go)); the days are *unverified* in a cluster |
| Does a deleted thread leave the backups early? | No. A backup cannot be edited; the data leaves when the backup that holds it expires. | by design of a physical backup |
| Are backups encrypted? | Not by this system: the content is as readable as the database is. People's own stored credentials (MCP servers) are sealed in the database (AES-256-GCM, [ADR 0046](decisions/0046-people-add-their-own-mcp-servers-secrets-in-a-credential-broker.md)) and their key is in the AWS secret, not in a backup. Encryption of the bucket at rest, who can read it and which region it is in are the owner's to set and state. | *unverified*: the bucket does not exist yet |
| What if the bucket keeps old versions? | Then a version barman deleted lives on until the bucket's own rule expires it, and the sentence above is longer by that. The owner sets that rule to fit the sentence. | the plugin's S3 advice, [object_stores.md](https://raw.githubusercontent.com/cloudnative-pg/plugin-barman-cloud/main/web/docs/object_stores.md), *verified 2026-10-17* |
| What if backups are switched off later? | Archiving stops and so does the sweep that expires backups: **what is in the bucket stays until someone deletes it.** | by the chart; *unverified* in a cluster |
| An erasure request? | The operator erases the person's threads from the live database (or the person deletes them). Backups cannot be edited: the answer is "they are gone from every backup within about 30 days". | the owner's to confirm |
| What else keeps a conversation? | The agent's own store of it (A2A has no task deletion: *unverified*), the model provider ([open question 45](open-questions.md)), and in an `agent-local` build adam's journal ([ADR 0043](decisions/0043-deleting-a-thread-erases-it.md), decision 7). | as in ADR 0043 |

## What the owner decides or confirms

- **28 days or fewer, and the sentence.** 28 days keeps the worst case at about 30 days; a shorter window (`21d`, `3w`) gives a margin. The sentence above says "about 30 days": change both together.
- **The bucket.** Its region, who can read it, whether it is encrypted at rest, and its versioning rule (above). AWS S3 `eu-central-1` was the recommendation; the preview RustFS in the cluster is the other shape tested by the plugin's authors.
- **Whether to tell testers about backups at all** before they exist: until `backup.enabled` is on, the honest sentence is that there are none.
- **The restore on netcup** is the proof that the backups can be used ([`deploy/chart/README.md`](../deploy/chart/README.md#backups)); until it has been done, nothing here is more than the chart's design.
