# `orbit-formal`: Kani Formal Verification Suite

Kani formal verification crate (`formal/Cargo.toml`) using **Kani Rust Verifier 0.68.0** (CBMC 6.11.0) to exhaustively verify Orbit's core security, state-machine, credential, and parsing invariants over bounded symbolic state spaces directly against the shared production `orbit-core` crate.

## Architecture

- **Zero FFI / runtime dependencies**: Both `orbit-core/` and `formal/` depend solely on the Rust standard library (`std`) with no Tauri, SQLite, tokio, serde, or OS-specific FFI dependencies.
- **Direct verification of production code (zero mirrored copies)**: Both `src-tauri/Cargo.toml` and `formal/Cargo.toml` depend on `orbit-core = { path = "../orbit-core" }`. Every Kani proof harness in `formal/src/*.rs` imports and proves the exact production functions from `orbit-core` that `src-tauri` calls and re-exports, eliminating model-to-code drift while keeping CBMC verification fast via bounded stack-allocated symbolic inputs.
- **Clean standard toolchain compatibility**: `formal/Cargo.toml` configures `[lints.rust] unexpected_cfgs = { level = "warn", check-cfg = ['cfg(kani)'] }` so `cargo check`, `cargo fmt --check`, and `cargo clippy -- -D warnings` pass cleanly alongside `cargo kani`.

## Modules & Proof Harnesses (20 Total)

### 1. `formal/src/path_safety.rs`
Directly verifies `orbit_core::path_safety` (called/re-exported by `src-tauri/src/harness/fs_guard.rs`, `harness/memory_links.rs`, `transcript/mod.rs`, `transcript/archive.rs`, `transfer/mod.rs`, and `transfer/resources.rs`).

| Harness | Property Proved |
|---|---|
| `proof_path_is_within_rejects_traversal_and_enforces_prefix` | Any `..` (`Parent`) component or symlink/reparse point is always rejected; acceptance implies strict lexical prefix containment under `root`; leading `..` on relative paths is preserved by `normalize_lexical_segments`. |
| `proof_is_safe_path_component_and_windows_device_rejection` | `is_safe_path_component_bytes` rejects empty, `.`, `..`, `/`, `\`, `:`, and `\0`; `validate_relative_path_bytes` / `is_valid_relative_component` rejects all Windows reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1` through `COM9`, `LPT1` through `LPT9`, any ASCII case, with or without extensions), ADS (`:`), traversal (`..`), and `.git`/`.orbit`. |
| `proof_backup_path_containment_under_backup_root` | A backup path is accepted only when neither it nor `config_root` contains `..` or links, and its resolved direct parent equals `<config_root>/.orbit/backups`. |
| `proof_codex_archive_path_and_image_extension` | `validate_codex_archive_path_bytes` accepts only `rollout.jsonl`, `rollout.jsonl.zst`, or `assets/<safe_name>.<ext>` with an allowed image extension (`png`, `jpg`, `jpeg`, `webp`, `gif`, case-insensitive), and rejects traversal, nested subdirectories, dotfiles, ADS, and non-image extensions (`svg`, `exe`). |
| `proof_collision_path_normalization` | When `case_insensitive` is true, ASCII-case-equivalent paths fold to identical keys and normalization is idempotent; when false, normalization is the exact identity function. |
| `proof_memory_link_tilde_and_leading_parent_normalization` | Empty and external scheme targets return `None`; `~` / `~/` expand against `home` (returning `None` if `home` is absent) and normalize without escaping `Root`; relative links with leading `..` preserve leading `..` components. |

### 2. `formal/src/state_machines.rs`
Directly verifies `orbit_core::state_machines` (called/re-exported by `src-tauri/src/ledger.rs` and `src-tauri/src/history.rs`).

| Harness | Property Proved |
|---|---|
| `proof_resolve_operation_state_invariants` | `Applied`/`None` rows always remain `OperationState::Applied`; `Pending` rows without a target resolve to `Drift`; `Pending` rows matching `after_checksum` resolve to `Applied` iff backup matches `before_checksum` (when backed up), else `Drift`; `Pending` rows matching `before_checksum` resolve to `NotApplied`, else `Drift`. |
| `proof_pending_was_superseded_invariants` | Non-`Pending` rows and rows without a target are never superseded; a `Pending` row at `index` is superseded iff some strictly later row (`j > index`) targets the same path. |
| `proof_classify_history_status_invariants` | Incomplete reservations always classify as `Legacy`; superseded pending rows always classify as `NotApplied`; remaining states map 1:1 (`Applied`, `NotApplied`, `Drift`, `Unavailable`); `Pending` is never emitted. |
| `proof_atomic_sequence_uniqueness` | Distinct monotonic sequence numbers `seq1 != seq2` at identical `(kind, id, action, stamp)` always produce distinct operation IDs (`format_op_id_buf`) and backup filenames (`format_backup_filename_buf`). |
| `proof_restore_and_undo_preconditions` | Restore/undo refuses stale target checksums, missing or mismatched backup preimages, legacy non-`tree-v2-sha256:` directory checksums, and incomplete/superseded/non-`Applied` rows; `RestoreReservation` enforces single-use `Idle -> Reserved -> Completed` transitions. |

### 3. `formal/src/credentials.rs`
Directly verifies `orbit_core::credentials` (called/re-exported by `src-tauri/src/harness/credentials.rs` and `src-tauri/src/harness/claude/mcp.rs`).

| Harness | Property Proved |
|---|---|
| `proof_credential_flag_requires_dash_prefix_and_no_positional_false_positives` | Positional arguments without a leading `-`/`--` never trigger `credential_flag_bytes`; bare `-`/`--`, invalid flag characters (`=`, space), and non-secret flags are rejected; valid secret flags are accepted case-insensitively. |
| `proof_scheme_and_sse_media_type_case_insensitivity` | `https://` scheme detection is ASCII case-insensitive; plain `http://` is allowed only for loopback hosts (`localhost`, `127.0.0.1`, `[::1]`) and never remote hosts; SSE `text/event-stream` media-type detection is case-insensitive across optional `;` parameters and rejects non-SSE types. |
| `proof_masked_placeholder_restoration_and_fresh_rejection` | Masked object fields matching a stored string restore the stored secret with no mask remaining; fresh placeholders (new keys or non-string stored values) are rejected with `FreshPlaceholderRemaining`; masks inside arrays are always rejected with `MaskInArray`. |
| `proof_secret_key_and_value_classification` | `secret_name_bytes` matches all 6 secret needles (`key`, `token`, `secret`, `auth`, `password`, `credential`) case-insensitively and rejects benign names; `value_shape_secret_bytes` detects `bearer ` (any case), `sk-`, `ghp_`, and `github_pat_`. |

### 4. `formal/src/transcript_and_parsing.rs`
Directly verifies `orbit_core::transcript_and_parsing` (called/re-exported by `src-tauri/src/history.rs`, `harness/skill_store.rs`, `transcript/spine.rs`, `transcript/mod.rs`, `transcript/archive.rs`, and `transfer/mod.rs`).

| Harness | Property Proved |
|---|---|
| `proof_parse_checksum_panic_free_and_roundtrip` | Arbitrary bytes at any digest offset round-trip identically through lowercase/uppercase hex encoding and `parse_checksum_bytes`; single-byte non-hex corruption, short inputs (`< 64`), and 64-byte multi-byte UTF-8 return `Err` without panicking. |
| `proof_extract_frontmatter_body_invariants` | Strips optional UTF-8 BOM (`0xEF 0xBB 0xBF`), handles LF/CRLF and trailing whitespace on `---` fences, rejects missing opening or closing fences, and returns the exact body without delimiters. |
| `proof_order_spine_nodes_cycles_uniqueness_and_parent_order` | Iterative spine traversal always terminates on arbitrary graphs (including self-loops and cycles), visits every node `0..n` exactly once with zero duplicates, and places parents strictly before children with `depth[child] == depth[parent] + 1` on root-reachable acyclic paths. |
| `proof_unique_title_collision_suffixing` | Always selects the smallest unoccupied slot (returning `source` when free, or `{source}-{k}` for the smallest free `k >= 1`) and never collides with `existing`. |
| `proof_bounded_decompression_and_truncation` | Bounded stream reads never consume more than `max_bytes + 1` bytes and accept iff `stream_len <= max_bytes`; bounded UTF-8 truncation always returns a valid UTF-8 prefix of length `<= max_bytes`. |

## Running the Suite

```bash
export PATH="/usr/local/cargo/bin:$HOME/.cargo/bin:$PATH"
export TMPDIR="$HOME/tmp"
./formal/run.sh
```

Run a single harness:

```bash
./formal/run.sh --harness proof_resolve_operation_state_invariants
```

Run formatting and lint checks:

```bash
cargo fmt --manifest-path orbit-core/Cargo.toml --check
cargo clippy --manifest-path orbit-core/Cargo.toml -- -D warnings
cargo fmt --manifest-path formal/Cargo.toml --check
cargo clippy --manifest-path formal/Cargo.toml -- -D warnings
```

## Mutation-Testing Verification Results

Every harness was validated by injecting a targeted bug into the verified logic, confirming `cargo kani --harness <name>` reports `VERIFICATION:- FAILED`, and reverting the mutation:

| Harness | Injected Mutation | Kani Result | Reverted |
|---|---|---|---|
| `proof_path_is_within_rejects_traversal_and_enforces_prefix` | Disabled `..` parent traversal check in `path_is_within_segments` | `VERIFICATION:- FAILED` | Yes |
| `proof_is_safe_path_component_and_windows_device_rejection` | Omitted Windows reserved `NUL` check in `is_windows_reserved_device_stem` | `VERIFICATION:- FAILED` | Yes |
| `proof_backup_path_containment_under_backup_root` | Ignored `backup_has_link` check in `backup_path_is_contained` | `VERIFICATION:- FAILED` | Yes |
| `proof_codex_archive_path_and_image_extension` | Skipped `is_safe_path_component_bytes` check in `validate_codex_archive_path_bytes` | `VERIFICATION:- FAILED` | Yes |
| `proof_collision_path_normalization` | Skipped ASCII lowercasing when `case_insensitive == true` | `VERIFICATION:- FAILED` | Yes |
| `proof_memory_link_tilde_and_leading_parent_normalization` | Dropped leading `..` on relative paths in `normalize_lexical_segments` | `VERIFICATION:- FAILED` | Yes |
| `proof_resolve_operation_state_invariants` | Skipped backup checksum match check in `resolve_operation_state` | `VERIFICATION:- FAILED` | Yes |
| `proof_pending_was_superseded_invariants` | Removed `LedgerStatus::Pending` guard in `pending_was_superseded` | `VERIFICATION:- FAILED` | Yes |
| `proof_classify_history_status_invariants` | Misclassified superseded pending row as `Applied` | `VERIFICATION:- FAILED` | Yes |
| `proof_atomic_sequence_uniqueness` | Hardcoded sequence suffix to `0` in `format_op_id_buf` | `VERIFICATION:- FAILED` | Yes |
| `proof_restore_and_undo_preconditions` | Disabled stale target checksum refusal in `check_restore_preconditions` | `VERIFICATION:- FAILED` | Yes |
| `proof_credential_flag_requires_dash_prefix_and_no_positional_false_positives` | Allowed positional args without leading `-` in `credential_flag_bytes` | `VERIFICATION:- FAILED` | Yes |
| `proof_scheme_and_sse_media_type_case_insensitivity` | Made SSE `Content-Type` check case-sensitive | `VERIFICATION:- FAILED` | Yes |
| `proof_masked_placeholder_restoration_and_fresh_rejection` | Removed post-restore `contains_mask` refusal | `VERIFICATION:- FAILED` | Yes |
| `proof_secret_key_and_value_classification` | Omitted `token` needle from `secret_name_bytes` | `VERIFICATION:- FAILED` | Yes |
| `proof_parse_checksum_panic_free_and_roundtrip` | Corrupted high-nibble shift (`<< 4` -> `<< 3`) in `parse_checksum_bytes` | `VERIFICATION:- FAILED` | Yes |
| `proof_extract_frontmatter_body_invariants` | Disabled UTF-8 BOM stripping in `extract_frontmatter_body_bytes` | `VERIFICATION:- FAILED` | Yes |
| `proof_order_spine_nodes_cycles_uniqueness_and_parent_order` | Disabled `seen[id]` cycle/duplicate guard in `order_spine_nodes_bounded` | `VERIFICATION:- FAILED` | Yes |
| `proof_unique_title_collision_suffixing` | Ignored title collision and always returned slot `0` | `VERIFICATION:- FAILED` | Yes |
| `proof_bounded_decompression_and_truncation` | Allowed off-by-one oversize stream in `read_bounded_stream` | `VERIFICATION:- FAILED` | Yes |

## Proof-to-Removed-Test Mapping

The following Rust unit tests in `src-tauri/src/**` tested pure invariants that are strictly subsumed by verified Kani 0.68.0 proof harnesses in `formal/` (proving `orbit-core` directly) and were removed to avoid redundant example-based checks while retaining all filesystem, SQLite, OS-symlink, process-lifetime, archive/conversion, and Tauri-boundary integration tests:

| Removed Rust Unit Test | Subsuming Kani Proof Harness | Why Covered |
|---|---|---|
| `harness::claude::mcp::tests::loopback_http_and_https_are_accepted_for_the_bench` | `formal/src/credentials.rs::proof_scheme_and_sse_media_type_case_insensitivity` | Exhaustively proves `is_loopback_http_url_bytes` and `is_allowed_mcp_url_bytes` accept `http://` only for loopback hosts (`localhost`, `127.0.0.1`, `[::1]`) and reject non-loopback HTTP/other schemes while accepting `https://`. |
| `harness::claude::mcp::tests::credential_flag_requires_dash_prefix_and_https_is_case_insensitive` | `formal/src/credentials.rs::proof_credential_flag_requires_dash_prefix_and_no_positional_false_positives`, `formal/src/credentials.rs::proof_scheme_and_sse_media_type_case_insensitivity` | Exhaustively proves positional arguments without a leading `-`/`--` never trigger `credential_flag_bytes`, valid secret flags match case-insensitively, and `is_https_scheme_bytes` is ASCII case-insensitive across all casing combinations of `https`. |
| `harness::claude::skills::tests::skill_ids_reject_windows_paths_ads_and_device_names` | `formal/src/path_safety.rs::proof_is_safe_path_component_and_windows_device_rejection` | Exhaustively proves `is_safe_path_component_bytes` and `is_valid_relative_component` reject empty names, `.`, `..`, `/`, `\`, `:`, trailing `.`/space, and all Windows reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1` through `COM9`, `LPT1` through `LPT9`, any ASCII case, with or without extensions). |
| `harness::claude::skills::tests::parse_frontmatter_handles_bom_and_trailing_whitespace` | `formal/src/transcript_and_parsing.rs::proof_extract_frontmatter_body_invariants` | Exhaustively proves `extract_frontmatter_body_bytes` strips an optional UTF-8 BOM (`0xEF 0xBB 0xBF`), handles LF/CRLF and trailing whitespace on `---` fences, and returns the exact body (YAML parsing remains covered by `inventory_reads_frontmatter_and_falls_back_on_malformed_yaml`). |
| `harness::kiro::memories::tests::steering_behavior_parses_crlf_frontmatter` | `formal/src/transcript_and_parsing.rs::proof_extract_frontmatter_body_invariants` | `steering_behavior` delegates frontmatter extraction to `skill_store::extract_frontmatter_body` (`orbit_core::transcript_and_parsing::extract_frontmatter_body`), whose CRLF fence handling is exhaustively proved by `proof_extract_frontmatter_body_invariants` (end-to-end steering inclusion remains covered by `steering_and_agents_files_have_link_graph_and_checked_delete`). |
| `harness::codex::skills::tests::path_is_within_rejects_parent_dir_traversal` | `formal/src/path_safety.rs::proof_path_is_within_rejects_traversal_and_enforces_prefix` | Exhaustively proves `path_is_within_segments` rejects any `..` (`Parent`) component and enforces strict lexical prefix containment under `root`. |
| `harness::memory_links::tests::resolve_reference_expands_tilde_and_preserves_leading_parent_dirs` | `formal/src/path_safety.rs::proof_memory_link_tilde_and_leading_parent_normalization` | Exhaustively proves `resolve_memory_link_segments` expands `~`/`~/` against `home` with lexical normalization and preserves leading `..` components on relative links. |
| `history::tests::parse_checksum_rejects_multibyte_utf8_without_panicking` | `formal/src/transcript_and_parsing.rs::proof_parse_checksum_panic_free_and_roundtrip` | Exhaustively proves `parse_checksum_bytes` round-trips arbitrary 32-byte digests in lowercase/uppercase hex and returns `Err` without panicking on short inputs, non-hex bytes, and 64-byte multi-byte UTF-8 sequences. |
| `ledger::tests::backup_paths_stay_under_the_backup_root_for_hostile_ids` | `formal/src/path_safety.rs::proof_backup_path_containment_under_backup_root` | Exhaustively proves `backup_path_is_contained` accepts backup paths only when they contain no `..` or links and their resolved direct parent equals `<config_root>/.orbit/backups`. |
| `ledger::tests::backup_path_and_op_id_are_unique_across_rapid_consecutive_calls` | `formal/src/state_machines.rs::proof_atomic_sequence_uniqueness` | Exhaustively proves distinct monotonic sequence numbers (`seq1 != seq2`) at identical `(kind, id, action, stamp)` always produce distinct operation IDs (`format_op_id_buf`) and backup filenames (`format_backup_filename_buf`). |
| `transcript::spine::tests::deeply_nested_chain_orders_iteratively_without_stack_overflow` | `formal/src/transcript_and_parsing.rs::proof_order_spine_nodes_cycles_uniqueness_and_parent_order` | Exhaustively proves iterative `order_spine_nodes_bounded` terminates on arbitrary graphs (including cycles), visits every node exactly once, and orders parents strictly before children with `depth[child] == depth[parent] + 1`. |
| `transfer::tests::normalize_collision_path_folds_case_for_case_insensitive_filesystems` | `formal/src/path_safety.rs::proof_collision_path_normalization` | Exhaustively proves `normalize_collision_bytes` folds ASCII-case-equivalent paths identically and idempotently when `case_insensitive == true`, and acts as the exact identity when `case_insensitive == false`. |
