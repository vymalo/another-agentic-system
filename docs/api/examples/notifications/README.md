# Notification vectors

What `orch_core::fold_notifications` makes of a thread's log (ADR 0077, slice N1). Each file is
`{name, role, after, events, expect}`: `events` are log events as the wire has them, `after` the
last `seq` already folded, `expect` the intents and withdrawals in order. The default expiry is 24 h
for a wait and 6 h for an end. `orchestrator/crates/core/tests/notify.rs` reads every file.

The web and the BFF read the same expectations.
