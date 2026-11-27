# Verification

`orbit-core` contains production rules that can run on data supplied by a caller. The desktop backend reads files, protects paths, takes locks, interprets provider formats, and persists state. Kani checks the shared Rust code on bounded inputs. TLC checks a finite journal model. Tests exercise the real boundary where the result also depends on the platform, storage, provider, or UI.

## Run the checks

Use Linux or WSL with Kani 0.68.0 (CBMC 6.11.0) and Java 21.

```sh
cargo install --locked kani-verifier --version 0.68.0
cargo kani setup
bash formal/run.sh

curl --fail --location https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar -o /tmp/tla2tools.jar
bash formal/run-tlc.sh /tmp/tla2tools.jar
```

`formal/run.sh` checks all names in [the Kani inventory](kani-harnesses.txt) and requires a successful result for every harness. Use `bash formal/run.sh --harness NAME` during local development. Run the full command and update the inventory when a harness changes. The TLC runner checks the jar checksum before running the [journal model](tla/README.md).

Run the core tests with `cargo test --manifest-path orbit-core/Cargo.toml` and the boundary suites with `cargo test --manifest-path src-tauri/Cargo.toml` and `npm test`. The [contribution guide](../CONTRIBUTING.md) describes optional native CLI integration tests.

## Coverage

Start with the required production behavior. For each guarantee, record the code that implements it, the check that exercises it, and its bounds or assumptions. Kani harnesses must call the production function. A TLA+ model must map its actions to the implementation, and a candidate implementation bug needs a replay through production code. Keep a test when it exercises a property, input range, platform, or caller that the formal check does not reach. When a check is replaced, record the replacement's coverage before removing the original.

| Production behavior | Kani or TLC coverage | Test coverage outside the formal scope |
| --- | --- | --- |
| Archive path validation and image allowlist | Kani checks arbitrary filename bytes through length 4, case variants of reserved Windows device names with an optional extension, and representative accepted and rejected archive layouts and image extensions. | Core tests check the 500-byte and 24-component boundaries and Unicode. Desktop tests exercise import against real manifests, files, checksums, and links. |
| Filesystem containment and backup paths | The shared path predicates cover portable syntax. | Desktop tests check canonical and missing paths, traversal, similar prefixes, symlinks, direct backup parents, constructor output for hostile IDs, and the Claude skill ID validator. |
| Memory links and collision keys | | Core tests exercise relative traversal, home expansion, external schemes, and Unicode case folding. Desktop tests exercise reference discovery and transfer conflicts. |
| Credential classification and MCP transport | Kani checks selected credential name and token forms, six-byte positional arguments and representative flags, and case variants of HTTPS and SSE media types. | Claude MCP tests use the parsed URL, check loopback addresses and remote rejection, and exercise response parsing and preservation of real JSON credentials during writes. |
| Journal classification and recovery | Kani checks the state classifier over optional checksum tokens and presence flags, supersession in journals of one to four rows, all history status combinations, and selected directory checksum pairs. TLC checks safety and conditional progress for up to three recorded creates, replacements, or deletions across two paths, including repeated contents. | Journal and history tests exercise real persistence, interrupted finalization, recovery across repeated contents, backup mismatch, stale checksum rejection, directory restore, and formatter output at integer boundaries. |
| Transcript parsing | Kani checks acceptance and exact decoding for every 64-byte checksum input, and rejection for arbitrary bytes at other lengths up to 65. It checks a frontmatter document of up to 16 bytes with optional BOM, LF or CRLF, fence whitespace, missing or invalid fences, and a one-byte body. | Core tests exercise the checksum string wrapper, malformed lengths 66–68, multiline Unicode frontmatter, an empty body, and a malformed opening fence. Provider tests exercise full transcripts, archive manifests, and indexing. |
| Transcript ordering, titles, and bounded reads | Kani checks the read probe cap for every `u64` budget. | Core tests exercise the actual ordering implementation on all 1,296 parent assignments in a four-node graph and a 20,000-node chain. They check multi-digit title suffixes and a reader's actual consumption and error propagation. Desktop tests check branch ordering, import collision handling, and decompressed byte caps. |

The Kani bounds are defined in [the harness source](src/). The TLA+ model's correspondence and assumptions are documented alongside [the model](tla/README.md). The suite has no Verus component. A claim about arbitrary graph sizes, journal schedules, or platform behavior needs evidence beyond the finite checks above.
