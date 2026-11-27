# Ledger write and recovery

`Ledger.tla` models up to three attempts to create, replace, or delete files at two target paths. One file starts absent and the other starts with checksum token 0. An attempt selects a path and a different state for that path, either absent or one of three checksum tokens. Attempts have distinct operation IDs and may request the same contents. An existing file is backed up before replacement or deletion. Creation has no prior bytes to back up.

| Model action | Rust operation |
| --- | --- |
| `Start` | [`ledger::recorded_mutation`](../../src-tauri/src/ledger.rs) allocates a fresh operation ID. [`atomic::write_recorded`](../../src-tauri/src/harness/atomic.rs) takes the mutation guard and prepares the plan. |
| `Reconcile` | [`ledger::reconcile_pending_for_target`](../../src-tauri/src/ledger.rs) considers only the newest reservation for the target. If it is pending and the observed target and backup match, it finalizes the row before appending the new reservation. |
| `Reserve` | [`ledger::reserve_row_locked`](../../src-tauri/src/ledger.rs) appends a Pending row after reconciliation. The row retains the original checksum. |
| `SaveBackup`, `Publish`, `VerifyReadback` | [`atomic::write_recorded` and `atomic::remove_current_recorded`](../../src-tauri/src/harness/atomic.rs) preserve existing bytes before publication. Creation and replacement verify the new contents. Deletion finalizes after a successful filesystem removal. |
| `Commit` | [`ledger::complete_mutation`](../../src-tauri/src/ledger.rs) marks the active reservation Applied. |
| `RequestObservation`, `Observe` | A caller requests [`ledger::read_applied`](../../src-tauri/src/ledger.rs). An Applied row or a matching, unsuperseded Pending row can appear in the result. The read leaves the journal unchanged. |
| `Interrupt` | A crash or terminal error ends the active attempt. The model retains the already-published journal, backup, and target state. |
| `Stabilize`, `Wait` | The environment can reach a period of sustained success. With no active attempt, the caller may remain idle indefinitely. |

The safety checks require a recorded reservation before publication and a matching backup when the file previously existed. Every Applied row must have published, each still-Pending published row must remain discoverable, and observations can only report requested publications. Each path must keep its original state until its first publication and match its latest publication thereafter. The Rust test `retry_does_not_credit_an_older_unpublished_write_for_matching_bytes` replays the counterexample where an attempt stops before publication and a later attempt writes identical bytes.

The liveness checks use weak fairness for each enabled internal step and for serving a requested successful read. Under sustained success, an active attempt eventually commits. An interrupted publication becomes visible or committed when a caller requests observation or starts a fresh same-target write while service is stable. The latter call can commit the old row during reconciliation even before reserving its own operation. A crash before publication can leave an abandoned Pending row. A crash after publication can leave a row Pending indefinitely if no caller observes it or starts another write. The model has no background worker or fairness assumption on caller requests.

TLC checks this finite model. The abstraction treats each completed storage publication as an atomic, retained effect and a history read as one observation. It excludes torn writes, power-loss persistence semantics, read interleavings, external writers, checksum collisions, no-op writes, directory moves, and malformed or legacy ledger rows.

From this directory, run TLC with `tlc2.TLC -config Ledger.cfg Ledger.tla`.
