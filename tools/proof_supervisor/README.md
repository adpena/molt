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
policy schema is `molt.proof-process-closure.v3`. It requires an absolute cwd,
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
  "schema": "molt.proof-process-closure.v3",
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
`state == "COMPLETE" && complete == true` and the replay-derived
`capability.admission.state == "admitted"`.

Both `verify` and `verify-rooted` return `receipt_sha256`/`receipt_bytes` and
`policy_input_sha256`/`policy_input_bytes` for the exact buffers decoded by the
native verifier. Python admission compares those fields with its own bounded
captures before accepting receipt semantics; a later pathname read is not the
verification result. `protocol.json` owns the 16 MiB policy input allowance,
enforced before parsing by every run, inventory and verification entrypoint.

Policy, receipt, event and export inputs use one retained direct regular-file
owner. Unix opens use no-follow and nonblocking flags before descriptor type
admission; Windows reuses the publication namespace owner, opens the final
reparse point itself and rejects reparse or non-disk handles. Reads stop at the
admitted extent plus one growth probe and check the retained generation and
current pathname before acceptance. Export performs that fence before emitting
its final footer. These bounds prevent special-file rendezvous and unbounded
append reads; they are not a deadline guarantee for arbitrary filesystem I/O.

Fixed-image path hashing and the macOS mapped-vnode pathname reader use that
same admission. Linux executable observation deliberately follows the kernel's
`/proc/<pid>/exe` magiclink; Windows observation receives the debugger's image
handle. Their platform identity checks remain authoritative, and the shared
executable hash cache bounds an uncached read by its initial seekable extent.
Directory sync and kernel process-metadata reads retain their distinct semantics.

## Compact durable evidence

`protocol.json` owns the policy, capability, receipt and event schemas. Cargo
generates Rust constants from it, and the Python custody consumer reads the same
file and includes it in the supervisor source identity. Capability v4 binds the
planned backend and required environment to one tagged `admission` value:

- `ineligible` includes a bounded nonempty `reason` and refuses before launch.
- `eligible` permits an attempt; it asserts no successful kernel admission.
- `admitted` includes `root_stable_process_id`, `root_create_sequence`, and
  `initial_image_sequence`. The shared process ledger derives this witness only
  after accepting the owned root and its policy-validated initial image.

Planners never emit `admitted`. Linux preflight can inspect Yama and its compiled
backend but cannot prove that an opaque outer seccomp policy permits the required
creation operations. Windows launch can also fail after an eligible plan. Failure
before the initial image therefore remains `eligible` with `INCOMPLETE`; later
failure remains `admitted` with `INCOMPLETE`. Ineligibility produces `REJECTED`.
Replay derives admission afresh and rejects a forged or partial witness. Windows
records genuine creation before fallible image observation, then exactly one
`initial-image` event for that live process. `process-create` has no image field.
Linux records root creation, descendant forks and exec events. These states describe developer proof custody, not
emitted program behavior or platform release qualification.

Terminal receipts use schema `molt.proof-process-closure-receipt.v6` and are
hard-limited to 65,536 bytes including the final newline. They keep bounded
diagnostic samples and counts, lifecycle, accounting (including `root_execs` and
`root_exit_terminated_processes`),
and evidence digests inline. Detailed process events are strict `ProcessEvent`
JSON objects, one per line, in a content-addressed adjacent artifact:

```json
{
  "event_log": {
    "schema": "molt.proof-process-event-log.v4",
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

The supervisor streams into a bounded direct-file temporary journal. Publication
syncs file contents, atomically renames, and syncs the parent directory (Windows
uses write-through replacement plus a flushed directory handle). The immutable
content-addressed event artifact is published first; the compact receipt is the
commit marker. Verification requires the deterministic adjacent filename,
digest, byte/record counts and contiguous sequence numbers. One `ProcessLedger`
applies typed process-create, initial-image, fork, exec, exit and unclassified-clone events
during both capture and replay. It validates the recorded platform dialect, stable process
identities, live parent ownership, image classification, root command, root exit,
derived-image stability and violation counts. Threads remain under kernel custody
without inflating the process ledger. Windows Job and completion-port accounting
remain independent kernel observations. Exact native path components, opened-file
identity and size are preserved in the image records.

`journal_coverage` and `native_custody` describe separate facts. Full coverage
can support a complete receipt only when replay and the native owner agree.
A refused observation freezes a typed prefix with its exact accepted sequence,
record count, byte count, digest and cause. Cleanup may close native custody while
that prefix still contains live processes; the receipt remains `INCOMPLETE`.
Windows preserves genuine debug creation/exit counters and raw held-Job totals,
including a reconciliation failure. It never invents creation, image or exit events
to make those counters agree. A partial/failed append poisons the journal and
prevents ordinary event or receipt publication.

The ledger borrows the sealed policy. Each transition validates budgets, reserves
index capacity and prepares owned keys/images before writing. Borrowed image and
event-wire preflight precedes classification and copies. A derived registry entry
owns its path once; full identity drift checks remain separate from path-key
equality. Retained admission identities and diagnostic samples participate in the
same aggregate debit as live images and derived witnesses. Each stored diagnostic
conservatively charges its full bounded buffer capacity; observations beyond the
sample cap store nothing and add no debit. Borrowed violation facts are formatted
only after the complete transition fits. Diagnostic formatting retains only its
bounded escaped-wire prefix.
Acceptance commits
only those prepared values; allocator observation tests cover the real append to
commit boundary. Hash indexes do not define wire order: retained identities are
sorted only when producing summaries. Cleanup terminal samples have fixed storage.

`protocol.json` is also the numeric budget authority. Its projections bound raw
policy input and canonical policy bytes (16 MiB each), receipt bytes (64 KiB),
one event (1 MiB), the journal (1 GiB / 10 million records), fixed rows (65,536),
distinct fixed paths and inventory identities (16,384), lifetime processes
(262,144), live processes (16,384), live trace tasks (65,536), retained ledger
payload (64 MiB) and retained derived identities (16 MiB). Scalar, role-group,
cache and diagnostic limits are in that same manifest. These are logical retained
storage and transport limits, not an RSS guarantee. Python publication counts the
exact compact UTF-8/LF encoding before allocating the document. Native readers
retain one bounded direct-file generation and fence mutation; canonical hashes
stream without retaining a second encoded policy. Alias groups retain every role
and reuse hashes for an opened identity/mutation generation while it remains in
the bounded cache (1,024 entries); aliases encountered after eviction can require
rehashing. The Python inventory has a distinct identity-count/transport bound;
the 64 MiB ledger payload bound does not describe Python receiver memory.

Native `verify` reports both `receipt_sha256` / `receipt_bytes` and
`policy_input_sha256` / `policy_input_bytes`. Consumers capture both file
generations before verification and bind the response to those retained bytes.
A successful integrity verification does not convert an incomplete receipt into
execution success.

### Apparatus capability matrix

| Native host cell | Implemented admission | Qualification boundary |
| --- | --- | --- |
| Linux little-endian x86-64 / AArch64 | clone3 pidfd root custody, ptrace creation/image stops, inherited audited seccomp creation filter | Requires kernel 5.3+, permitted syscalls/Yama policy and native regression/performance receipts for the exact host/profile; no blanket signal or supervisor-death claim |
| Windows x64 / ARM64 | suspended DEBUG_PROCESS, retained root handle and nested kill-on-close Job, genuine create then initial image | Native debug/Job reconciliation and performance receipts are required for each host/profile |
| macOS, all modes | prelaunch ineligible refusal | No complete process/image custody backend is qualified |
| Other Linux supervisor ABIs | prelaunch ineligible refusal | No audited creation filter is implemented |

An eligible plan authorizes an attempt, never a verified platform claim. Linux
TRACEME group stops explicitly refuse; the backend does not implement SEIZE/LISTEN
parity. Root clone3 and the inherited clone3 restriction are intentional apparatus
contracts and must be compatible with the selected proof workload. These host
restrictions do not alter compiled guest semantics or Molt target support.

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
  Application SIGTRAP and other delivery stops retain the subject signal.
  A detected job-control group stop refuses with incomplete evidence and cleanup:
  TRACEME cannot use the SEIZE-only `PTRACE_LISTEN` operation to preserve it.
  This implementation does not silently swallow the stop or switch backends.
- macOS refuses every mode as `ineligible`. The former Seatbelt/ptrace leaf
  implementation could miss re-exec when the subject blocked SIGTRAP; it has
  been retired. Tree modes also lack retained pre-entry creation authority.
  [HF-07](../../docs/agent/V1_HANDOFF_FINDINGS.md) owns the open qualification
  contract. Enabling a mode requires independent pre-entry creation/image,
  signal-behavior and supervisor-death controls; an entitlement or a positive
  preflight alone does not establish them. This is a developer proof boundary,
  not a restriction on the compiler's macOS target support.

Receipts follow one enforced lifecycle:
`CREATED -> POLICY_SEALED -> RUNNING -> DRAINING -> COMPLETE|INCOMPLETE`, with
`POLICY_SEALED -> REJECTED` for ineligible plans. The adjacent artifact
contains process/image events only, and the receipt keeps their aggregate
reconciliation; neither serializes ambient process or module inventories.

Repeated executable classification uses a per-run bounded hash cache keyed by
stable OS file identity plus a mutation token from the same open handle. Linux
uses device/inode with size/mtime/ctime; Windows uses volume/file index with
size, last-write time, and non-user-restorable change time. Identity and token
are re-read after hashing (and on hits); any change fails closed.


### Retained guest roots and export

`verify-rooted --rootfs DIR --policy FILE --receipt FILE` verifies Linux guest
policy/receipt/event identities against retained filesystem bytes without
launching anything. Fixed leaf/declared-tree policies are supported; inventory
and derived roots fail. Guest logical paths and policy hashes are unchanged;
all executable and cwd components must be exact regular files/directories with
no links or traversal aliases. A validated offline policy cannot be launched.

`run-export --policy FILE --receipt FILE` runs the same supervisor and exports
its exact terminal receipt and events after guest stderr, followed by the final
length footer declared in `protocol.json`. Stdout is unchanged. Protocol limits
bound receipt/event payloads, not arbitrary guest output. Receivers must bind
raw export bytes to retained evidence and run the native verifier. Verification
success means consistency; workload acceptance additionally requires COMPLETE,
zero root exit, no errors/violations, closed accounting and the owning command,
output, root and provider checks. Release replay owns the complete filesystem
and trusted engine boundary; the supervisor alone does not establish it.
