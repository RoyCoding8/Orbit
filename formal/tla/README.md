# Ledger write and recovery

`Ledger.tla` models the recorded replacement of an existing file. The TLC configuration allows three fresh attempts on one target, each with a different operation ID and after-checksum. All attempts request a backup. The initial target has checksum token 0, and attempt `a` requests checksum token `a`.

| Model action | Rust operation |
| --- | --- |
| `Start` | [`ledger::recorded_mutation`](../../src-tauri/src/ledger.rs) allocates a fresh operation ID. [`atomic::write_recorded`](../../src-tauri/src/harness/atomic.rs) takes the mutation guard and prepares the plan. |
| `Reconcile` | [`ledger::reconcile_pending_for_target`](../../src-tauri/src/ledger.rs) treats a pending row with matching target and backup as applied and finalizes it before appending the new reservation. At most one pending row can match in this model because after-checksums are distinct. |
| `Reserve` | [`ledger::reserve_row_locked`](../../src-tauri/src/ledger.rs) appends a Pending row after reconciliation. The row retains the original checksum. |
| `SaveBackup`, `Publish`, `VerifyReadback` | [`atomic::apply_and_write`](../../src-tauri/src/harness/atomic.rs) saves the original bytes, replaces the target, and verifies the result. |
| `Commit` | [`ledger::complete_mutation`](../../src-tauri/src/ledger.rs) marks the active reservation Applied. |
| `RequestObservation`, `Observe` | A caller requests [`ledger::read_applied`](../../src-tauri/src/ledger.rs). An Applied row or a matching, unsuperseded Pending row can appear in the result. The read leaves the journal unchanged. |
| `Interrupt` | A crash or terminal error ends the active attempt. The model retains the already-published journal, backup, and target state. |
| `Stabilize`, `Wait` | The environment can reach a period of sustained success. With no active attempt, the caller may remain idle indefinitely. |

The safety checks require a recorded reservation and matching backup before publication. Every Applied row must have published, each still-Pending published row must remain discoverable, and observations can only report requested publications.

The liveness checks use weak fairness for each enabled internal step and for serving a requested successful read. Under sustained success, an active attempt eventually commits. An interrupted publication becomes visible or committed when a caller requests observation or starts a fresh same-target write while service is stable. The latter call can commit the old row during reconciliation even before reserving its own operation. A crash before publication can leave an abandoned Pending row. A crash after publication can leave a row Pending indefinitely if no caller observes it or starts another write. The model has no background worker or fairness assumption on caller requests.

TLC checks this finite model. The abstraction treats each completed storage publication as an atomic, retained effect and a history read as one observation. It excludes torn writes, power-loss persistence semantics, read interleavings, external writers, checksum collisions, repeated after-checksums, no-op writes, creates, deletes, directory moves, and malformed or legacy ledger rows.

From this directory, run TLC with `tlc2.TLC -config Ledger.cfg Ledger.tla`.
