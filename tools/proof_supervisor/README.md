# Molt proof supervisor

`molt-proof-supervisor` is the native process-image closure authority for proof
commands. It launches exactly one content-addressed policy and writes exactly
one content-addressed terminal receipt. Runtime hooks may improve diagnostics;
they are never closure authority.

## Build and invoke

```text
python tools/proof_supervisor/build.py --release
molt-proof-supervisor capability leaf
molt-proof-supervisor run --policy policy.json --receipt receipt.json
molt-proof-supervisor verify --policy policy.json --receipt receipt.json
```

The standalone crate is intentionally outside the main Rust workspace. The
policy schema is `molt.proof-process-closure.v2`. It requires an absolute cwd,
an absolute command image, an exact environment, a fixed SHA-256 image set,
and optional non-overlapping derived executable roots. `leaf` rejects every
descendant process. `declared-tree` admits only fixed images or identities first
executed from declared derived roots; a derived path cannot change identity
during the run.

### Cold Cargo custody and preserved candidates

The Python proof queue owns prelaunch derived-root provenance through
`proof_queue_pkg/cargo_cache_custody.py`. An admitted Cargo target is fresh,
empty, and exclusively locked. Completed, drained, input-stable supervision can
seal its output as a preserved candidate, including after completed test failures.
Timeouts, incomplete process closure, or changed inputs never seal it. Unsealed
generations remain evidence and the next attempt starts a distinct cold generation.

Warm admission is not implemented: process supervision and Git source capture do
not enforce all filesystem/environment inputs of build scripts or procedural
macros. A seal proves captured output, not complete compilation-input closure.
Encountering a sealed candidate rejects execution with the classified
`cargo-input-closure-unproven` diagnostic, including its path and seal reference.
The candidate and state pointer remain unchanged; rejection does not reread the
large target or output manifest. The runner also rejects asserted reused custody.
No warm-cache speed gain or rebuild-topology optimization is claimed by this lane.

The consumer-neutral `molt.file_locks` module owns OS and in-process locking;
compiler build-path policy remains in `molt.cli.build_locks`. Proof cache
admission does not import the CLI or frontend merely to obtain a lock.

Candidate identity binds independently captured Git-tracked and nonignored untracked
source bytes/modes, explicit overlays, the Git revision, captured toolchain files,
command/profile/target and deterministic semantic environment. Queue nonces and
effective output paths are execution transport, not a proven compilation closure.
Existing developer targets are not imported. Candidate output capture rejects
links, junctions, special entries, external hard links and changes during inventory.

Source capture never recursively inventories ignored local caches. Explicit
overlays and toolchain inputs extend captured source, but no declaration alone
proves the absence of undeclared reads. Future warm admission requires an enforced
complete-input authority, not an allowlist promise or cached source assertion.
The current execution receipt independently owns the source-content CAS reference
and its terminal handle/topology verification, so a seed cannot assert its own
source identity. Output inventory uses the shared no-follow topology and
handle-bound hashing authority with a closing membership fence. Work is linear
in admitted source bytes and selected target bytes; source capture reports its
file count, bytes hashed, and wall time separately from target selection.

`cargo_target_selection` records requested and effective target, cold state,
immutable input references, and selection wall time before the proof
command starts. The runner validates that same custody authority against the
native policy. The Rust supervisor continues to bind every actually executed
derived image to its content identity; declaring a directory alone is not the
queue's cache admission proof.

Rust tests that produce linkable or executable fixtures use
`runtime/test_support/cargo_test_artifacts.rs`. The helper creates a uniquely
named directory beside the canonical running Cargo test image and never falls
back to system TEMP. Cargo already resolved the invocation's selected target;
the test must not reinterpret a relative `CARGO_TARGET_DIR` from its package cwd.
The supervisor remains the authority for admission within the captured absolute,
exclusive Cargo target. At launch it hashes the opened executable and binds its
OS file identity and mutation token; directory placement is not content proof.

Generated inputs and images remain Cargo-owned after each test, preserving bytes
for terminal receipt capture and replay. Tests never recursively delete fixture
directories by a potentially replaced pathname. The existing selected-target
custody and retirement lifecycle owns eventual cleanup; ordinary source-only
Cargo runs retain these outputs under the normal Cargo target lifecycle. There
is no fixture-specific cleanup protocol or independent retirement registry.

Fixed-image paths are canonicalized only for identity admission. The exact
lexical `command[0]` is preserved for launch and argv0 semantics (for example,
Rustup's `cargo`, `rustc`, and `rustup` proxies). Multiple policy rows that
resolve to one exact image contribute a sorted role set; event identities expose
`roles: [...]` instead of inventing one ambiguous role.

The command envelope constructs the policy directly from its compact capture
summary and artifact manifest:

```json
{
  "schema": "molt.proof-process-closure.v2",
  "nonce": "128-or-more-bits-of-hex",
  "mode": "declared-tree",
  "cwd": "absolute captured cwd",
  "command": ["absolute root executable", "arg"],
  "environment": {"EXACT_CAPTURED_KEY": "value"},
  "root_role": "cargo",
  "fixed_images": [
    {"role": "cargo", "path": "absolute cargo image", "sha256": "64 hex"},
    {"role": "rustc", "path": "absolute rustc image", "sha256": "64 hex"},
    {"role": "linker", "path": "absolute linker image", "sha256": "64 hex"},
    {"role": "linker-auxiliary", "path": "absolute helper image", "sha256": "64 hex", "root_exit_disposition": "terminate"}
  ],
  "derived_roots": [
    {"role": "build-script", "path": "absolute captured target root"}
  ]
}
```

Fixed images default to `root_exit_disposition: require-exit`. Only an exact,
hash-sealed auxiliary may declare `terminate`; the supervisor ends those
processes after the root command exits and records the count in
`accounting.root_exit_terminated_processes`. Any non-auxiliary descendant still
live at root exit is a closure violation, not implicit cleanup.

Platform auxiliaries use the same fixed-image contract. On Windows,
declared-toolchain execution captures the single native console broker returned
by `GetSystemDirectoryW` as `windows-console-broker`, seals its exact path,
SHA-256, and size before custody arms, and revalidates those bytes after the
run. The system directory is never a derived root, and neither a basename nor a
directory allowlist is process-image authority. Leaf execution admits no
platform auxiliary.

`run` exits 0 only for a receipt with `complete: true`, 78 for a sealed
incomplete/rejected receipt, and 2 for malformed input. `verify` requires the
exact policy and independently canonicalizes it, revalidates fixed images, and
binds its policy and nonce digests to the receipt. It also replays every
supervisor-state and process-event transition, rejects unknown JSON fields,
reconciles accounting and derived-image identity, and checks receipt/event
content identities. It exits 0 for any authentic terminal receipt (including an
authentic `INCOMPLETE`/`REJECTED` receipt) and 79 for a well-formed but invalid
receipt. Integration must require successful verification plus
`state == "COMPLETE" && complete == true`.

## Compact durable evidence

Terminal receipts use schema `molt.proof-process-closure-receipt.v3` and are
hard-limited to 65,536 bytes including the final newline. They keep bounded
diagnostic samples and counts, lifecycle, accounting (including `root_execs` and
`root_exit_terminated_processes`),
and evidence digests inline. Detailed process events are strict `ProcessEvent`
JSON objects, one per line, in a content-addressed adjacent artifact:

```json
{
  "event_log": {
    "schema": "molt.proof-process-event-log.v1",
    "file": "receipt.json.events.<sha256>.jsonl",
    "count": 42,
    "bytes": 8192,
    "sha256": "64 hex"
  },
  "derived_image_summary": {"count": 3, "sha256": "64 hex"},
  "violation_count": 0,
  "error_count": 0
}
```

The supervisor streams into a bounded buffered temporary journal. Publication
syncs file contents, atomically renames, and syncs the parent directory (Windows
uses write-through replacement plus a flushed directory handle). The immutable
content-addressed event artifact is published first; the compact receipt is the
commit marker. Verification requires the deterministic adjacent filename,
digest, byte/record counts, monotonically increasing sequence, legal process
lifecycle, stable process identities, and an identical recomputed derived-image
summary.

## Kernel authority

- Windows starts suspended under `DEBUG_PROCESS`, assigns a nested
  kill-on-close Job before release, classifies each `CREATE_PROCESS_DEBUG_EVENT`
  image handle before entry, drains debug exits, and reconciles Job accounting.
  Debug-event waiting is intentionally infinite: the outer proof guard owns the
  wall-clock timeout, and terminating the supervisor closes the Job and kills
  its entire tree. Only each process's initial loader breakpoint is debugger-
  handled; application breakpoints retain normal Windows semantics.
- Linux uses `PTRACE_TRACEME` with fork, vfork, clone, exec, exit, and `EXITKILL`
  events. Exec images are classified at the kernel stop before user code runs.
  An unreadable `PTRACE_EVENT_CLONE` thread-group identity is a terminal
  violation, and a run cannot complete without an admitted root exec event.
- macOS rejects before launch because this binary has no entitled Endpoint
  Security helper. It does not substitute polling or kqueue for recursive
  process-image authority.

Receipts follow one enforced lifecycle:
`CREATED -> POLICY_SEALED -> RUNNING -> DRAINING -> COMPLETE|INCOMPLETE`, with
`POLICY_SEALED -> REJECTED` for unavailable capabilities. The adjacent artifact
contains process/image events only, and the receipt keeps their aggregate
reconciliation; neither serializes ambient process or module inventories.

Repeated executable classification uses a per-run bounded hash cache keyed by
stable OS file identity plus a mutation token from the same open handle. Linux
uses device/inode with size/mtime/ctime; Windows uses volume/file index with
size, last-write time, and non-user-restorable change time. Identity and token
are re-read after hashing (and on hits); any change fails closed.
