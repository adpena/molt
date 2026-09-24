# Proof Queue Agent Contract

`tools/proof_queue.py` is the custody boundary for expensive, contentious, or
long-running Molt proof work. It serializes lanes by contention key, records the
exact command and git snapshot, writes guarded logs, enforces proof DAG
dependencies, and projects each noted or linked run into a deterministic marimo
notebook for collaborative inspection.

## Registered source-extension producers

`source-extension-produce` submits one registered package/version/module-set and
target-Python/ABI/target cell. Use its `--help` for the shared producer options;
`--print-spec` resolves the address without provisioning, and `--queue-only`
performs setup then queues without starting the proof. The registry, not this
command's parser, owns which version/platform/architecture cells are admitted.
Named submissions normalize the target triple for replay.

Standalone and queued setup share read-only output-topology validation: the
build root must be fresh, and source, build and publication/candidate roots must
be disjoint. Candidate-only custody is resolved before setup. Queued preparation
revalidates these paths; checking a fresh root never creates its parent.
Mutable publication and recovery state is still checked under the owning lock.

One shared preparation boundary validates the pinned upstream checkout,
provisions and verifies its recursive submodules, and provisions the
content-addressed source-build environment through the canonical uv authority.
The realized environment must retain its planned root, interpreter and manifest
addresses. Recipe runtime capture uses the shared isolated base-interpreter
probe, not the launcher's imported native modules; planning and realized
environment probes share their capture imports. Distribution import roots
belong to environment custody, not the runtime tree. Standalone `produce-set`
and `attest-set-candidate` launchers use the
same preparation boundary and then re-execute once with `--prepared`.
Proof execution starts directly from that interpreter with `-P -m molt.cli`
and requires `--prepared`. This typed precondition permits no submodule repair,
environment provisioning or interpreter restart; it does not skip verification.
Both prepared build modes validate the active locked environment and the pinned
source/submodules before destination mutation or publication locking. Git
inspection disables optional index writes. Missing or stale prerequisites fail
without repair under proof custody. One typed invocation owns CLI
options, locked re-execution, and queue argv. Envelope v4 declares Python, Git,
and the target-derived compiler family; ordinary Python proofs remain leaves.
Persisted command envelopes, execution requests and supervisor policies/receipts
use the exact-JSON codec end to end; duplicate keys and non-finite numbers are
rejected before admission or evidence interpretation, not collapsed by a decoder.
Provider v2 records lexical compiler entrypoints, content images, target commands,
the WASI sysroot manifest and the selected compiler-builtins archive. Capture
uses the explicit selected environment; validation consumes recorded identities.
The child consumes the captured archive through the queue-owned link-input
contract instead of rediscovering it. The typed link-input contract lives in
`molt.source_extension_link_inputs`; its CLI resolver alone selects tools and
archives. Both enter source custody, while ordinary proof-cache imports do not
load the CLI or frontend. Preconfigured compiler arguments share a
positive grammar: unknown or external-input/helper selectors fail before probes.
Response-file and un-inventoried launcher forms fail explicitly. Tool-family identity alone does not replace
the producer's source/input/seal validation or prove an ecosystem matrix cell.

The queue publishes `MOLT_PROOF_SOURCE_ROOT` only from its validated Git snapshot.
Users cannot override it. The Python bootstrap exposes that same checkout's
`src` only to the typed Molt module payload, including under `-P`; unrelated
module, script, directory/ZIP, command and stdin import behavior is unchanged.
Source, sysroot, compiler-builtins and executable inputs enter live custody
before execution. Native requests preserve host CC/CXX selection separately
from their recorded effective target triple.

The executable is intentionally only a stable source-checkout entrypoint. The
canonical implementation lives in `tools/proof_queue_pkg/`: `state` owns the
SQLite schema, paths, rows, notes, DAG, and serialized mutex facts; `custody`
owns process identity and queue-owned process control; `scheduling` owns atomic
admission and leases; `diagnostic_engine` preserves classifier order while the
`diagnostic_*_rules` modules own their queue/build/link/runtime rule families;
`diagnostic_evidence`, `diagnostic_model`, `diagnostic_audit`, and
`diagnostic_reporting` own live evidence, shared values, audit/frontier logic,
and human rendering respectively; `evidence` owns notebook and JSON projections;
`policy` owns command/toolchain admission; `runner` owns guarded execution;
`commands` orchestrates CLI operations; and `pact` owns the named scientific
witness lanes. Pact imports are lazy so status/help and normal queue work do not
load NumPy/SciPy witness tooling. New behavior belongs in its owning module;
`tools/proof_queue.py` must not become a compatibility facade or re-export
internal implementation symbols.

Build-capacity admission is synchronous and read-only, owned by
`src/molt/disk_capacity.py`. Queue admission, guarded Cargo setup, generation
acquisition, and the actual native/WASM Cargo execution consumer use this same
threshold and receipt schema. The default minimum is 25 GiB; an explicit
`MOLT_DISK_GUARD_HIGH_WATER_GB` must be positive and finite. Unknown capacity,
invalid policy, or insufficient space rejects before launch with the measured
path/free/required bytes. Tests inject measurements; pytest and cleanup-disable
flags never waive admission. This is a launch floor, not a reservation or a
promise that an arbitrarily large build will fit.

Reclamation is separate: `tools/disk_guard.py` owns its existing narrow
artifact allow-set. A reclaim plan or projected byte count is not admission.
Environment selection and shell hooks do not launch an additional detached
disk guard. Never broaden the generic allow-set to proof Cargo generations:
their run ownership, exclusive lease, terminal receipts, and retained evidence
belong to `cargo_cache_custody`. Disk failures are reported as
`build-disk-capacity`, not requests to change compiler semantics. Inspect
structured rejection evidence before reclaiming; source/WIP, active targets,
uncertain owners, reusable sealed candidates, and prior policy-denied paths stay
intact. Successful non-reusable candidates require the explicit terminal-sealed
retention policy below; generic reclamation cannot adopt them.

Cargo output environment is owned by `cargo_output_environment.CargoOutputEnvironment`.
The admitted Cargo operation selects the same documenter requirement used by tool
capture. Policy is carried explicitly through identity, acquisition, binding,
and final validation; execution argv may already contain Python custody wrappers
and must not be reparsed as a new logical command. The parent derives its policy
independently from the original admitted envelope, not the child's policy claim.
The exact transformed execution argv still participates in input identity.
The input identity represents `CARGO_TARGET_DIR`, and documentation
commands' `TEMP`/`TMP`/`TMPDIR`, as typed build-output bindings; the lease binds
their actual values before returning its execution environment. Other commands'
caller temporary paths remain semantic inputs. Both pre-execution validation and
the parent terminal verifier require the exact leased paths, so symbolic identity
does not admit a redirected or missing output variable. There is no post-identity
temporary-directory rewrite. Input schema v2 makes the changed policy explicit;
old receipts are not rewritten or retroactively made valid.

Inspect one generation with `uv run --python 3.12 python tools/proof_queue.py
reclaim-cargo-generation --run-id RUN_ID`. This emits JSON and does not create
queue state, hash build outputs, or delete files. Add `--apply` only for an
authorized cleanup. The command requires a persisted terminal queue result,
its terminal digest, and a matching immutable generation receipt. Missing
legacy ownership is retained, never inferred or adopted by this command.
Unattested failures without a valid terminal queue digest also remain outside
this command's cleanup authority. `inspection-failed` / `not-authorized` report
that authority could not be established, not that artifact presence was proved.

A sealed candidate is not part of unsealed reclamation. Inspect one with
`uv run --python 3.12 python tools/proof_queue.py
retire-terminal-sealed-generation --run-id RUN_ID`. It becomes eligible only
when the persisted terminal row, digest, immutable Cargo lifecycle receipt,
owner binding, process closure, and the existing per-identity lease all agree;
the publication must be the non-reusable
`cargo-input-closure-unproven` preserved-candidate form. The default policy admits
only failed terminal runs. An explicitly audited older successful run may be
selected with `--allow-passed`; this does not admit reusable output or weaken any
custody check. Select exact run IDs, preserve current proof/replay artifacts, and
inspect the complete cohort before applying it. There is no age/LRU sweep or
automatic successful-output retirement. `--apply` first
records append-only intent, then under the same `target.lock` captures the
output manifest and timings into custody CAS before retiring only the target.
It preserves the source/toolchain inputs, terminal receipt, publication seal,
owner tombstone, and outcome note. The selected status policy is recorded with
the inspection, intent, and retirement evidence and is revalidated under the
identity lock on apply. Successful targets without the opt-in, reusable, active,
linked, ambiguous, legacy, indeterminate, and prior-blocked targets are retained. A
failed deletion or interrupted retirement is `retire-blocked` and is never
automatically retried. The original immutable lifecycle projection remains
`terminal-sealed-retained`; `retired-sealed` is a later observed owner/pointer
lifecycle, not a rewrite of the proof result.

Each Cargo generation owns an `owner.json` under its exclusive identity lease;
`state.json` is only a latest-generation navigation pointer. Closing a lease
records publication but does not authorize reclamation. The parent validates
guard and execution custody before binding terminal generation evidence.
Reclamation revalidates the exact persisted terminal reference under the same
identity lock, accepts only terminal-unsealed output with proven process
closure, and preserves output manifests, timing files, terminal receipts, and
an owner tombstone. Sealed candidates remain retained, without warm reuse until
complete Cargo input closure is enforced, except for an explicitly applied
terminal-sealed retirement that revalidates terminal custody and preserves its
receipt and output inventory first. Interrupted or failed reclamation becomes
`reclaim-blocked`, and interrupted or failed sealed retirement becomes
`retire-blocked`; neither is an automatic retry. Inspecting an owner is an
observation; apply always revalidates. Queue notes preserve cleanup intent and
outcome without rewriting the original proof receipt.

The actual command result and the final queue outcome are distinct authorities:
a command can exit zero while dirty source makes the proof `non-evidence` with
queue return code 2. Decide that final outcome before binding the parent terminal
digest or Cargo generation receipt. Generation terminal schema v2 binds both
facts, including the typed `molt.proof-queue-terminal.v1` outcome; reclamation
checks the final outcome against the persisted queue row without replacing the
actual command return code. Parent-added outcome data participates in the parent
terminal digest, not the already-sealed child execution digest. Old terminal
schemas remain retained rather than inferring new outcome fields or rewriting
their original receipts.

Process launch options come from the typed `src/molt/process_spawn.py` authority,
shared by queue custody, the memory guard, and pytest bootstrap. Keep explicit
launch arguments and text streams typed through their consumers. Named proof
environments are parsed by `policy` into string tables and case-folded locked
names before admission; receipt objects and string lists are validated by
`runner` before they reach custody verification. Malformed input must produce a
policy or receipt diagnostic, never an unchecked cast or incidental type error.
`pact.NamedProofSpec` types all built-in named lanes; queued and detached modes
share one submission path, and detached dispatch closes its database connection
on both success and failure. Process cleanup lives in
`memory_guard_core.process_custody`; guard entrypoints must not rebind that
module's callbacks. Tests inject samplers or patch the owning module directly.

Native supervisor capability v2 owns the required launch environment. The queue
reads it from the captured supervisor binary before toolchain, process-image,
and source capture; both inventory and proof policies seal the effective values.
Native policy admission rejects missing or conflicting requirements. Windows
requires `_NO_DEBUG_HEAP=1`: debugger-based process observation must not enable
heap debug checks or disable the low-fragmentation heap. `DEBUG_PROCESS`, job
containment, pre-entry image admission, and descendant accounting remain active.
Other platforms advertise their own requirements rather than inheriting a
Windows setting. Do not replace this contract with a host environment tweak.

Executable and derived-root identities use canonical native paths at live
filesystem boundaries. Safe Windows prefix simplification must preserve device
namespaces, trailing-dot/space semantics, and native code units. Identity keys
and component containment do not infer case or Unicode equivalence; the
filesystem supplies canonical spelling. Evidence recording, sorting, and replay
use those recorded identities without consulting the current filesystem.

The native supervisor is itself a debugger. Its kernel tests must own that
debugger boundary, not run inside another recursive debugger: nested debuggers
can hide descendant events from the outer supervisor while job accounting still
counts them. Build the standalone test targets through Cargo queue custody
(`--manifest-path tools/proof_supervisor/Cargo.toml`, `--no-run`), then execute
the exact built test images with the existing memory guard and record their
content identities and results. Keep build custody and kernel-test receipts
distinct. An incomplete nested queue receipt is never acceptance, even if an
inner test succeeds; do not weaken process accounting to make it pass.

Rust linker custody follows the selected compiler's host-tool search, including
the selected sysroot and compiler sysroot `lib/rustlib/<host>/bin` directories.
The compilation target does not own these executable tools. Explicit linker
paths remain exact. Cargo cross-compiles additionally select native host-unit
linkers for build scripts and proc macros; capture these through Cargo's real
host proc-macro semantics with the original target/configuration, not by
reconstructing host flags or admitting every installed linker. Receipts retain
each unit's selection provenance and frozen images; verification rehashes those
images without repeating compiler selection. Missing custody fails before the
requested build rather than falling back to PATH changes or copied aliases.

Guard scratch is owned by `src/molt/temporary_artifacts.py`. The parent allocates
one short `pt-*` directory before child launch and passes it through
`MOLT_GUARD_SCRATCH_ROOT`; pytest and guarded helpers consume that allocation.
Keep human-readable run/platform identity in receipts, not every scratch path:
native compiler/linker descendants still have classic path-length limits.
Nested guards rebind an inherited guard-default pytest root to their new lease;
explicit test/temp roots remain caller-owned. Managed UV environments use the
durable `<artifact-root>/uv-project-envs/<purpose-python-source-key>` namespace;
an explicit `UV_PROJECT_ENVIRONMENT` is honored without moving existing data.

The parent holds an OS lock and its original allocation identity through closure.
Shared lock files are content-neutral: OS lock arbitration precedes any protected
work, and unused PID publication cannot race a contender's lock initialization.
Windows requires completed empty-Job accounting; POSIX records sampled/process-
group closure with a final sample and positive-bounded liveness probe, not a
kernel-equivalent tree guarantee. Indeterminate closure preserves the allocation.
After proven closure the parent exclusively retires the payload into its own
`gs/<guard-token>/payload`. Only this nested payload is reclaimable from persisted
receipts; forged metadata cannot redirect cleanup to a legacy sibling `pt-*`.
Clean success reclaims; failed closed runs retain up to three eligible payloads
within 2 GiB. Busy, blocked or invalid custody stays protected and is reported;
contention can defer the retention bound until a later completion. A pending
index discovers unfinished retention work without rescanning all historical
receipts, but grants no deletion authority. Interrupted deletions are never
retried: absence repairs the receipt, surviving payloads become blocked.
Index publication stages stay inside the locked generation; the shared pending
namespace contains only complete entries. Discovery snapshots are reconciled
under each generation's lock before reading the index or counting retained
bytes. A disappeared entry is accepted only when verified terminal custody proves
that generation was reclaimed; missing retained-owner indexes and malformed
entries remain failures, never existence-check retries or ignored corruption.
Owner/terminal/error receipts survive payload cleanup. Guard summaries and command
profiles expose outcome, evidence path and finalization time; elapsed command
time includes cleanup. `child_returncode` records the actual child result;
`infrastructure_failure` records independent scratch-custody failures. Such a
failure changes child success to guard status 125 (`infrastructure_error`), not
137/SIGKILL, and never replaces a nonzero child status. Both facts survive captured
and streamed harness results, command profiles, incident summaries and metrics.
Differential consumers retain that typed failure through CPython and every backend:
it is uncalibrated infrastructure evidence, never semantic parity, an expected
language failure, OOM inferred from an exit code, or permission for a cold retry.
Cargo execution preserves the same fields in each attempt and never retries a
wrapper or bisects test failures on infrastructure evidence. Benchmark and
calibration consumers exclude these runs from performance or conformance claims.
Restored benchmark summaries derive metrics from normalized runner outcomes;
serialized aggregate values cannot reintroduce rejected timings or speedups.
Queue receipts bind guard and child return codes independently while retaining
the existing process-closure and sampling checks. Every accepted guard receipt
requires the shared descendant-closure result to be closed with its direct child
reaped; an infrastructure incident cannot mask an indeterminate POSIX final sample
or failed cleanup action. Infrastructure failures produce
queue status `non-evidence` for successful children, or `failed` for unsuccessful
children, with queue code 2 and an infrastructure diagnostic in either case.
The sealed command result and guard receipt retain their actual return codes;
an infrastructure diagnostic never enters the semantic-failure frontier.
Nested Cargo test runners stop semantic attribution on the same typed outcome.
Their explicit log reports are diagnostic observations only, never authenticated
queue acceptance or process-cleanup authority.
No age/LRU janitor adopts legacy scratch or environments.

The disk space required to complete a build is a first-class optimization target,
not only final binary size. Prerequisite footprint, successful cold/warm peak
working storage, retained output, reusable cache and retired generations are
separate metrics. A post-failure inventory is not a build-peak measurement.
Record wall-clock and rebuild tradeoffs with any storage reduction; deleting warm
artifacts is not a demonstrated reduction in the space required to build. Do not
lower admission headroom to hide unknown demand.

File-launched tools bind imports through `tools/import_file.py` before loading
repository helpers. Already-executed foreign packages or descendants are errors;
unexecuted namespace search paths are bound to the selected source, including
resource lookup and parent-first restoration. Git hooks use this same authority,
not independent `sys.path` rewrites or bypasses.
The pre-push hook sets the selected worktree's native `PYTHONPATH` before Python
startup and uses `uv --no-project --offline --no-config` to run an installed
interpreter without synchronizing environments. Sharing an interpreter does not
authorize loading the main checkout's source. Hook refreshes retain foreign-hook
chains across idempotent installation and source updates.

Python startup guarding is owned by `src/molt/pytest_memory_guard_bootstrap.py`;
state paths are owned by `src/molt/memory_guard_paths.py`. Current-test
allocation, active-guard markers, child validation, and parent repro
reads derive their paths from the effective command environment at use time,
never import-time directory constants. Hosted CI custody projection can occur
after guard modules import; the parent and child must still select the same root.
The current-test snapshot names the active or last-observed node, not prior
failures. Portability CI uses unbuffered verbose pytest output so completed
node outcomes survive in the CI transcript even if timeout prevents the final
summary. Full tracebacks may still require replay of the named failed nodes.
Source and test-local `sitecustomize.py` files are adapters into that package,
not repository-wide import-path authorities. Non-test startup must leave the
checkout root absent unless the caller already selected it. Only a confirmed pytest, test-module, or
direct-test invocation may expose repository tooling and enter the existing
memory-guard handoff. Keep this distinction intact: making the whole checkout
importable forces isolated Python custody to inventory artifacts and unrelated
WIP as executable input.
Running and terminal guard summaries share `reporting.GuardReportContext`.
Win32 process-query signatures come from `tools/windows_process_api.py`; query-only
consumers share its cached table, while consumers binding additional APIs own
their DLL table and apply the shared binder. This preserves pointer-width handles
without allowing one consumer's ctypes structures to overwrite another's ABI.
Release and source identity probes use the canonical command execution boundary
with finite deadlines. The raw-call audit follows callable aliases and static
`getattr` capability queries as well as direct calls; lookup alone is not execution.

## When To Use It

Use the queue for Cargo builds, WASM/browser proofs, benchmark lanes,
conformance shards, stress tests, and any command likely to contend for shared
build/runtime resources. Direct commands are still appropriate for cheap source
inspection, changed-file formatting, static checks, narrow unit tests, and
queue/bootstrap repair. For Rust, use `tools/dev.py fmt-check` or
`tools/check_rustfmt.py --changed`; write mode compares `rustfmt --emit stdout`
before touching files and keeps generated Rust under generator custody.

The compiler-authorities shard records Cargo's built-in `--timings` report in
its run-owned target under `cargo-timings/`. Use that unit timeline for compile
critical-path attribution before changing crate boundaries or parallelism.
Process-custody events prove process identity and lifecycle, not per-crate
wall time; their sequence numbers must not be interpreted as timestamps.

`rust.test.ir-wasm-runtime-authorities` selects the IR/pass and WASM families
alongside runtime call/frame/namespace/object ownership tests, without requesting
native/Rust/Luau code generation or ignored runtime GC benchmarks. Runtime
families include arena cleanup, sealed attribute layouts and builtin class
publication/rollback; these sibling contracts run in the same built image.
Its captured
Node and WASM-linker tools remain required for actual WASM consumers. The
compiler-authorities command depends on this batch and owns the complementary
native/Rust/Luau, IR and lowering tests; it does not repeat pass/WASM libtests.
Use both command receipts when claiming complete compiler-family acceptance.
Neither correctness batch replaces optimized-runtime proof.
Both batches stream libtest diagnostics with `--nocapture`: a later runtime
abort must not discard the earlier assertion details needed to classify the
whole failure family. Retain the detailed run log and query it on failure;
console summaries need not repeat passing test output.

Public ownership/memory pass contracts live in the `ownership_memory_contracts`
Cargo integration target and link the ordinary `molt-passes` library; private
analysis/kernel tests remain in libtest. The core batch selects both targets,
with the integration modules retaining the `tir::` family namespace. Keep target
selection and filters together when changing test topology so coverage cannot
silently disappear. Compare Cargo timings only under matching profiles, source
inputs, cache state, and toolchains; test counts are not a wall-clock prediction.

Receipt unit tests use `tests/proof_queue_custody_test_support.py`: real Python
validation, CAS hashing and custody binding over synthetic test inputs, with
only the exact native-verifier execution boundary replaced. They never build
or launch a supervisor. The same substitution assertions run against the real
supervisor in `tests/tools/test_proof_queue_native_receipts.py` (`slow`), selected
by the existing warm native-integration batch in `tools/proof_plan.toml`.

Execution receipts have one compact wire representation, capped at 64 KiB.
`execution_receipt_details` publishes complete child-policy/event inventories,
environment inventories (with their existing value-HMAC policy), and prelaunch
custody-authority inventories into the existing `custody-cas` store. The
`execution_details` reference binds the full detail to the run and execution
nonce; each replaced inventory carries its exact type, count, and content hash.
Nothing is sampled, clipped, or omitted to fit the cap. Toolchain and native
supervisor custody keep their existing authorities.

For an agent query, load the execution JSON and call
`execution_receipt_details.expand_context(record["receipt_context"],
cas_root=execution_path.parent / "custody-cas")`. This verifies the CAS bytes,
run/nonce binding, and exact projection closure before returning full detail.
Runner admission and Cargo candidate sealing use the same expansion authority;
missing, substituted, or legacy inline detail fails closed. Custody digests
cover the compact wire context, not a separately serialized expanded copy.
Source eligibility decisions and live mutation evidence are unchanged: receipt
compaction never converts dirty or transiently mutated inputs into reusable
cache proof.

`ProofPlan.inventory_hash_workers` owns hashing concurrency for both Python/toolchain
and Git-source inventories. Source telemetry retains total `capture_s` and the shared
file-capture `inventory_profile` (worker count, files, bytes, hash time), so hashing
can be distinguished from enumeration, metadata checks, fences, and CAS publication.
Worker count never changes content identity or weakens mutation detection.

Before queueing, always inspect live custody:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py status
```

On this Windows workstation, expensive queue rows must refresh the canonical DX
environment before submission so `C:\Molt` is the selected artifact and
toolchain authority:

```powershell
$dx = python tools\run_context_env.py --prefer-external-artifacts --dx --format powershell
Invoke-Expression ($dx -join [Environment]::NewLine)
```

This bootstrap intentionally does not use `uv`: on an explicit exFAT fallback
checkout, `UV_LINK_MODE=copy` must be exported before uv creates or syncs
`.venv`, otherwise uv first attempts hard links and emits slow fallback noise.
Use an already-installed host Python 3.12+ for this dependency-free resolver
script; after the env is imported, use `uv run --active --project . --python
3.12 ...` for project commands. In `--dx` mode the resolver emits one durable,
source-keyed project environment under `<MOLT_EXT_ROOT>/uv-project-envs/`, so
repeated checks in one checkout reuse the same uv environment instead of
creating per-session environments. A caller-owned explicit
`UV_PROJECT_ENVIRONMENT` is preserved. Do not run two uv bootstrap/sync commands
in parallel in the same fresh checkout; one process owns project-environment
creation.

The healthy default is `MOLT_EXT_ROOT=C:\Molt`,
`CARGO_TARGET_DIR=C:\Molt\target`, and
`MOLT_TARGET_ROOT=C:\Molt\target-root`, with `UV_PROJECT_ENVIRONMENT` stable at
`C:\Molt\uv-project-envs\<dx-python-source-key>` for the standard Python 3.12
DX lane.
Rows with any canonical run root on `D:` fail closed; there is no preservation
flag or volume-label fallback. `MOLT_EXT_ROOT` may be explicitly configured for
non-custodial output on approved volumes, but named inputs, package seals,
worktrees, and `MOLT_TARGET_ROOT` remain rooted at `C:\Molt`.

Hosted CI is a typed exception to checkout *location*, never to canonical local
custody. A GitHub-hosted source checkout may physically live under runner
storage such as `D:\a\...`, but it is source-only. The workflow issues a
per-run `MOLT_CI_EPHEMERAL_CUSTODY_ROOT` under `RUNNER_TEMP`; Molt accepts it
only when the reserved GitHub repository, workspace, workflow, event, commit,
run, OS, and architecture facts agree. A lone `CI`, `GITHUB_ACTIONS`, or Molt
environment switch cannot self-attest. Package seals, build environments,
proof state, caches, and test scratch then resolve beneath that disjoint per-run
root rather than the source checkout. On hosted Windows, managed toolchains use
the separately verified `RUNNER_TOOL_CACHE` and still reject `D:`; other hosted
platforms use the per-run custody root. Outside that verified contract, a `D:`
checkout still fails closed exactly as local policy requires.

Deterministic child environments retain `CI`, `RUNNER_TEMP`, `RUNNER_OS`,
`RUNNER_ARCH`, and the GitHub facts needed to revalidate that contract; filtering
them out must not silently turn an admitted runner into an unowned local path.
The guarded command's working directory is separate from its Molt source
authority. Staged packages and standalone Cargo projects use the guard tools
belonging to the loaded Molt checkout, not tools discovered in the command
directory. A foreign preloaded guard or guard dependency fails before launch;
it is never replaced while it may hold live process custody. An installed package
without the owning source guard tools reports that requirement explicitly.

`C:\Molt` is the artifact and warm-checkout tier, not a disposable cold-clone or
backup treadmill. Create a new `C:\Molt\worktrees\...` checkout only for real
isolation from dirty WIP or branch surgery; for read-only doc/status checks
prefer `git show origin/main:<path>` from the canonical checkout, and for
repeated proof work reuse the warm checkout plus queue-assigned roots. Harvest
unique signal by reviewed cherry-pick/pathspec landing onto `main`, then delete
the source worktree/branch/bundle rather than preserving backup piles.

Do not use the queue as proof theater. Submit the narrow proof that covers the
changed contract, then return to structural work.

Compiler build rows share a queue-owned `compiler-build-resource` mutex on this
Windows/C:\Molt workstation. `rust`, `native-build`, `queue-native-rust`,
`wasm`, and `wasm-browser` resource families, plus `cargo:*`, `rust:*`,
`wasm:*`, and `wasm-browser:*` contention keys, may keep different
human-facing contention keys for lane identity, but they must not overlap
rustc/Cargo/backend build work just because the keys differ. This protects the
host from cold-build resource failures such as Windows `os error 1450` while
writing Rust bytecode under `C:\Molt\target\...`. The queue derives the
shared mutex itself; do not bypass it with a hand-chosen key or a raw background
command.

## Cargo Proof Lanes

The canonical `rust.test.compiler-authorities` command is a correctness lane.
Its inline `dev-fast` override builds the Cranelift codegen dependency at host
optimization level zero; backend-generated program optimization is unchanged.
Keep this exact command in proof-plan custody and measure total build-plus-test
time. Its receipt does not prove optimized-host compiler performance; performance
claims require the corresponding production profile.

Cargo proofs use the queue-native `cargo` subcommand. Do not submit raw
`cargo ...` through `exec`, the TOML DSL, shell backgrounding, or a Codex-held
interactive session. The cargo lane builds the canonical command envelope:
active uv with `--no-sync --no-config` for the internal runner,
`tools/guarded_exec.py --prefix MOLT_TEST_SUITE`, queue contention, memory
guard, timeout, logs, optional detached runner, and a Cargo contention key
inferred from `-p/--package` when one is present. A cargo row that spends its
proof budget rebuilding or syncing the Python project before Cargo starts is a
queue DX regression.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py cargo `
  --id runtime-buffer-descriptor-authority `
  --reason "Prove typed storage exports one runtime-owned buffer descriptor" `
  --scope runtime/molt-runtime/src/object/memoryview.rs `
  --note "Moved buffer descriptor authority beside TypedStridedStorage; proving C API and ABI layout stay aligned." `
  --timeout 900 `
  --detach `
  -- test -p molt-runtime buffer --lib -- --nocapture
```

Use `--contention-key` only when the inferred `cargo:<package>` or
`cargo:workspace` key is not precise enough for the shared artifact cache and
compile slot being protected. Use `cargo-template` to print the current command
shape instead of reconstructing it from memory:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py cargo-template
```

## Required Submission Shape

Every queued run needs a meaningful reason, resource family, contention key,
scope, and note. The note should say what changed or what is being tested or
explored and why.

For `exec` and `cargo`, the `--` delimiter before the proof command is
mandatory. The queue rejects any positional token before that delimiter because
it means shell quoting likely broke a metadata value such as `--reason` or
`--note`; running anyway would silently drop scope, contention, notes, detach
mode, or timeout authority. `exec --help` and `cargo --help` are parser help
and do not require a delimiter; `--help` after `--` remains an argument to the
proof command.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py exec `
  --id runtime-buffer-descriptor-authority `
  --reason "Prove typed storage exports one runtime-owned buffer descriptor" `
  --resource-family python `
  --contention-key python:runtime-buffer-descriptor `
  --scope runtime/molt-runtime/src/object/memoryview.rs `
  --note "Moved buffer descriptor authority beside TypedStridedStorage; proving C API and ABI layout stay aligned." `
  --timeout 900 `
  -- uv run --active --project . --python 3.12 --no-sync --no-config pytest tests/path.py -q
```

Use `--depends-on RUN_ID` when a proof is not valid until earlier evidence has
passed. Dependency edges are immutable, acyclic, and queue-enforced: a child
waits while parents are queued/running and becomes `blocked` if a parent has
already failed or gone stale. A blocked row is scheduling evidence, not a lost
proof log: the queue writes a small blocked log, keeps the DAG parent visible in
`evidence`, and reports the deterministic `proof-dependency-blocked` diagnostic.
`run`, `status`, `prune-stale`, `evidence`, `audit`, `diagnose`, `notebook`,
and new submissions reconcile impossible queued dependencies before reporting
or enforcing contention; do not launch a worker just to clear a dead dependency
row.

Queue commands that invoke Python must use:

```powershell
uv run --active --project . --python 3.12 --no-sync --no-config ...
```

Non-active `uv run` is rejected because it creates throwaway environments and
destroys proof latency.

### Command, Toolchain, Environment, and Source Custody

Each row persists one typed command envelope at admission. It contains the
exact argv, a closed Python, toolchain, or generated-proof-plan kind, the complete
interpreter-affecting `uv run` prefix, Windows `py` selector, and non-empty
declared toolchains. The generated command registry covers every proof-plan
argv; identical matrix argv retain every owning command ID and must agree on
one toolchain set. A plan entrypoint with one registered argv is exact-only, so
changing one selector cannot downgrade its Cargo, Node, Quint, Lean, or other
declared closure to generic Python custody. Shared entrypoints such as pytest
remain typed command families. There is no untyped or empty-toolchain executable lane:
unknown binaries reject before execution. Opaque shells, unmodeled `uv`
options, ambiguous console scripts, and noncanonical Rust wrappers are also
rejected before execution. The same recursive parser recognizes
relative and absolute `tools/guarded_exec.py` plus
`python -m tools.guarded_exec`, binds the delegated executable and every
requested toolchain, and rejects a second delegation layer. `uv --with`,
`--with-editable`, `--with-requirements`, environment files, indexes, and
find-links are forbidden: a `uv run` overlay is an ephemeral environment
whose interpreter has no stable image to admit. Pact witness preparation
provisions the typed locked environment of the `pact-witness` dependency group
before proof custody, with pins bound to `config/scientific_stack_versions.toml`.
The proof starts directly from that inventoried interpreter; it does not
provision or relaunch an environment inside the guarded execution.

Every envelope also carries one constructional process closure. Exact plan and
guarded typed-delegation commands may launch only their declared, content-bound
toolchains. Non-exact Python and Node commands are no-descendant leaves: Python
audit custody and a queue-owned Node preload hook reject Cargo, Node, shell,
fork, and other undeclared launch paths before spawn and persist every attempt
in the receipt. A non-exact native launcher has no pre-spawn authority and fails
at launch, requiring the guarded typed command family—even a version command
may be a shim that starts the resolved tool. This is one shared rule for every
launcher family, not a per-command allowlist.

Named Python lanes use the same prepared, typed environment inventory as
source-extension proofs. The registered payload and arguments are bound to
the exact interpreter, and its recursive input and toolchain closure is captured
before launch. A directory name or an environment manifest alone grants no
permission to start a new child image. Undeclared executables fail before spawn
with the refusal retained in the receipt.

Every proof run also receives one fresh, custody-external scratch root in
`MOLT_PROOF_SCRATCH_ROOT` (a run-owned derived root beside the run's Cargo
target, role `scratch-output`, empty at launch and receipted). Witness outputs
may use that root without becoming watched source inputs. Source-extension
production still requires explicit package/version, Python, source, and fresh
build-root selection through its typed preparation boundary. Scratch allocation
does not authorize replacing a prior build tree or weakening source custody.

Tools that ship as prebuilt release binaries (`wasm-tools`, `sccache`, `node`)
are pinned in `config/tool_releases.toml` by version-addressed asset URL,
byte size and SHA-256, each under a declared provenance (a GitHub release
record or an official `SHASUMS256.txt` checksum manifest).
A toolchain policy that cites that manifest as setup evidence makes the queue
provision the host asset under `<toolchain root>/toolchains/<name>-<version>`
(digest-verified, attested, idempotent) and place its `bin` first on the lane's
PATH before toolchains are located, so the version policy always meets the
pinned release rather than an ambient install; the CI workflow pin is gated
against the same manifest.

Toolchains whose selected launcher starts a distinct executable declare bounded
`process_image_probes` in `tools/proof_plan.toml`. Before source custody arms,
the native supervisor runs each exact probe in non-evidence inventory mode and
records every kernel-observed executable by path, size, and SHA-256. That sealed
image set is the single authority consumed by both pre-spawn child custody and
the proof supervisor; no install-directory or basename allowlist is inferred.
Inventory is lossless on Windows and Linux. macOS remains fail-closed until an
entitlement-backed Endpoint Security process backend is available.

One queue-owned memory guard contains interpreter/tool identity probes,
toolchain preflight, the proof command, and both source snapshots. The guarded
child resolves the exact outer and payload executables and records their paths,
sizes, and content hashes. Every declared proof-plan toolchain binds its policy,
version, launcher, resolved content executable, configuration files, and PATH
child closure as applicable. Rustup shims bind the selected binary, Node binds
its runtime versions/configuration/global paths, Quint binds the resolved npm
package tree, and environment-selected compiler/linker/wrapper executables are
content-hashed. All toolchains are re-captured after the command; a missing,
empty, changed, or extra closure cannot become evidence.

Cargo build-script header discovery is independent of Rust linker selection.
The queue pins a selected Clang driver through `CLANG_PATH` and independently
binds available `llvm-config` through `LLVM_CONFIG_PATH` before capturing the
execution environment. Bindgen's formatter is independently bound through
`RUSTFMT`; Rustup proxies use the same content-proven resolver as runtime
compiler/Cargo planning, including symlink aliases while preserving custom
binaries. The typed leading `cargo +toolchain` selector is projected into the
effective `RUSTUP_TOOLCHAIN` before all build-tool and Rust toolchain capture;
it takes precedence over an inherited selection without changing Cargo argv.
These tools use the ordinary environment-executable identity
and supervisor projection, including lexical and resolved paths; there is no
separate Clang image allowlist. Explicit hooks require absolute executable paths
because Cargo build scripts do not share the invocation cwd. Target-specific
bindgen arguments and both dynamic/static libclang path hooks remain semantic
inputs. An unavailable optional selector retains its diagnostic transcript and
does not require an unused capability from pure Rust builds. A missing optional
Rustup formatter component is recorded without selecting an alternate tool;
an explicit formatter hook or malformed successful selector still fails.

Conventional Cargo configuration discovery is shared with runtime build planning,
including default Cargo home and extensionless-config precedence. Cargo-owned
`[env]` overrides of driver/discovery hooks that cannot be resolved before
capture reject explicitly; this is not a claim of complete Cargo environment
precedence support. Full libclang/header content closure is also separate from
this executable-process contract.

The queue resolves its guard budget through the shared
`harness_memory_guard.limits_from_env("MOLT_PROOF_QUEUE", ...)` authority. Use
`MOLT_PROOF_QUEUE_MAX_PROCESS_RSS_GB`,
`MOLT_PROOF_QUEUE_MAX_TOTAL_RSS_GB`,
`MOLT_PROOF_QUEUE_MAX_GLOBAL_RSS_GB`,
`MOLT_PROOF_QUEUE_CHILD_RLIMIT_GB`, and
`MOLT_PROOF_QUEUE_MEMORY_GUARD_POLL_SEC` for queue-specific control; the
corresponding global `MOLT_MAX_*`, `MOLT_CHILD_RLIMIT_GB`, and
`MOLT_MEMORY_GUARD_POLL_SEC` names remain lower-precedence fallbacks. Resolved
values are validated, clamped by live and hard custody ceilings, and recorded
in the exact guard command.

Python custody additionally binds the venv launcher and `pyvenv.cfg`, base
CPython executable and shared libraries, stdlib and native-extension byte
manifest, resolved runtime/import roots, and installed distributions. Every
RECORD path retains its lexical and resolved path, owning root, relative path,
symlink state, and containment classification. A RECORD path must resolve under
the interpreter install prefix or an explicitly admitted editable/source root;
path traversal and external symlink escapes reject before hashing. Declared
RECORD hashes and sizes are verified, and editable source bytes, commit, and
tree are bound. The deterministic sorted
file worklist uses the proof-plan worker bound and one streaming read per unique
resolved identity; prelaunch and postcompletion both read the complete byte
inventory. `--directory` and `--project` may not escape the admitted source
root.

Endpoint hashes are backed by live kernel mutation custody during the command.
Windows `ReadDirectoryChangesW` and Linux inotify watch the admitted Git files,
runtime roots, package trees, executables, and configuration bytes;
queue overflow or watcher failure is fail-closed. A write, replacement, rename,
delete, or metadata change remains an event even if the command restores the
original bytes before completion, so mutate-execute-restore cannot produce
eligible evidence. Platforms without a lossless implemented watcher reject
execution rather than falling back to sampling.

The proof command receives only classified host/runtime, compiler, and Molt
environment names. Ambient pytest injection, package-index controls,
unclassified overrides, URL credentials, and secret-bearing names are rejected
or omitted. Queue-owned `PYTHONDONTWRITEBYTECODE`, `PYTHONNOUSERSITE`, and the
Node global-search-path policy are canonical inputs; `uv --no-config` prevents
user or host configuration from silently changing resolution. Receipts store
names, classes, and keyed fingerprints for every
passed value, never plaintext values; queued logs and notebooks expose override
names only.

Receipts bind run ID, a fresh execution nonce, row and effective cwd, Git root,
commit and tree, cleanliness/status digest, environment,
toolchains, and executable identities at both prelaunch and postcompletion.
Stdout and stderr are streamed to byte-hashed artifacts and recognized test
commands must publish structured result counts. A passed row additionally
binds the exact terminal memory-guard receipt and clean cleanup outcome into one
terminal-evidence digest; sampler enforcement must be complete with no transient
gaps. Stale execution JSON or guard summaries, substituted receipts, child
signals, or missing counts cannot become evidence. A dirty, unavailable, or changed source;
changed editable distribution, toolchain, environment, or executable
is terminal `non-evidence`, even if the command returned zero. Terminal
projections read the persisted context and never re-probe an ambient host.
Command argv, cwd, and envelope are immutable admission columns enforced by the
database. Normal reconnect never derives a new envelope from the current plan;
queued plan drift fails validation at launch, while terminal rows are never
rewritten. Export requires the row envelope and its digest to equal the envelope
inside the terminal receipt before returning evidence.
Legacy rows are marked `legacy-unattested`; migration derives an admission
envelope but never fabricates historical execution evidence.

## Detached Long Runs

Do not hand-roll background proof launchers with PowerShell `Start-Process`,
shell-specific quoting, or Codex interactive sessions. The queue owns detached
launch:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py exec `
  --id runtime-buffer-descriptor-authority `
  --reason "Prove typed storage exports one runtime-owned buffer descriptor" `
  --resource-family python `
  --contention-key python:runtime-buffer-descriptor `
  --scope runtime/molt-runtime/src/object/memoryview.rs `
  --note "Detached queue-owned runner for the focused buffer proof." `
  --timeout 900 `
  --detach `
  -- uv run --active --project . --python 3.12 pytest tests/path.py -q
```

Named lanes support the same mode:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py pact-witness-acceptance --detach
```

Detached submission creates a queued row, starts a queue-owned runner for that
exact run ID, marks the row `dispatched`, and prints both the run ID and
`*.runner.log`. The runner then uses
`tools\proof_queue.py run --run-id RUN_ID`, so it cannot steal a different
queued row. `dispatched` is active queue custody: it consumes queue capacity and
prevents duplicate launch until the runner claims the row as `running` or
`prune-stale` reclaims an expired handoff. WASM resource families also preflight
the checked-in Rust toolchain contract and install/check required Rust targets
before Cargo starts.

Use the queue-size scheduler instead of launching several detached rows by hand:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py run `
  --detach `
  --queue-size 3
```

`--queue-size N` is the maximum number of concurrently `dispatched` or
`running` rows across all contention keys. The default is `1`; set
`MOLT_PROOF_QUEUE_SIZE=N` for a shell/session default. `run --detach` defaults
its launch limit to the queue size, while `--limit` remains a per-invocation cap.
Queued rows are wait-list state and do not consume capacity; launch uses an
atomic queued-to-dispatched claim that rechecks global capacity and contention
key ownership immediately before spawning a detached runner. The scheduler skips
rows whose contention key is already active or already selected in the same
batch, so increasing queue size only admits independent work. Build-heavy
families (`native-build`, queue-native Rust/Cargo, `wasm`, and `wasm-browser`)
also share the `compiler-build-resource` mutex: only one such row may be
dispatched or running at a time even when their contention keys differ, while
light rows with disjoint families can still use remaining queue capacity.
`running-proof-launch-summary-stale` is diagnostic evidence only while the
queue-owned guard is still live; it means the memory guard has not yet published
child-process custody. Only terminal stale signals such as
`running-proof-child-missing` and `running-proof-guard-timeout-expired`, a
dead/reused guard, an expired dispatch handoff, or the fallback running-age
ceiling may reclaim a live row. The timeout signal is emitted when a non-final
`running`/`child_running` summary remains past its own typed `limits.timeout_s`
contract plus the bounded finalization grace. Its evidence records row age,
timeout and overdue durations, guard PID, marker state/age, and exact
summary/log artifacts even when the log has no child output. This keeps
Windows, macOS, and Linux detached rows from being marked stale just because an
old queued log or launch summary predates the current execution epoch while
making violated guard contracts directly actionable.
Queue-owned uv subprocesses default `UV_LINK_MODE=copy` unless the operator
already set a value. This keeps APDataStore, exFAT, cross-device caches, and
other valid Windows/macOS/Linux storage layouts out of noisy hardlink fallback
paths without disabling cache reuse or overriding an explicit operator choice.

The source checkout also exposes a shell-free convenience front door. Prefer
this for interactive use because it is the portable command surface:

```shell
molt queue --queue-size 3 run --detach
```

`molt queue ...` forwards to `tools/proof_queue.py` using Python argv lists, not
a shell. The top-level `--queue-size N` is a portable per-invocation shorthand
for `MOLT_PROOF_QUEUE_SIZE=N`; it avoids PowerShell/Bash/Fish-specific
environment syntax while leaving scheduling and validation in
`tools/proof_queue.py`. The wrapper rejects invalid top-level capacity before
spawning the queue process, so bad values do not leak as latent environment
state. Use either that shorthand or `run --queue-size N`, not both. The command
surface also installs the canonical Molt DX environment around the child queue:
`C:\Molt` artifact roots, target/cache/temp roots, `MOLT_TARGET_ROOT`, and
`UV_LINK_MODE=copy` flow through the same RunContext authority as other build
wrappers. When a warm project environment is visible, `molt queue` preserves it
as `UV_PROJECT_ENVIRONMENT` instead of creating a fresh session venv. Resolution
is explicit and ordered: existing `UV_PROJECT_ENVIRONMENT`, active
`VIRTUAL_ENV`, checkout-local `.venv`, main-worktree `.venv`, then `MOLT_VENV`.
This keeps the NVMe root warm: use `C:\Molt` for shared build/cache/toolchain
state, not as disposable cold worktree or backup churn.

The command surface is the same on Windows, macOS, and Linux, and it must not be
replaced with PowerShell-specific launch wrappers, POSIX backgrounding, or
shell-quoted command reconstruction. Raw `uv run ... tools/proof_queue.py`
examples below remain source-checkout diagnostics and CI/bootstrap forms; they
are not a second queue authority. The portability tests intentionally include
spaces and shell metacharacters in paths/arguments, plus Windows and POSIX
detached-runner assertions; update those tests with any queue launch change.
The canonical proof plan selects its generated `platform_portability` matrix
whenever queue launch or checkout-custody surfaces change. The single CI
executor expands that authority into Linux, macOS, and Windows cells and emits
one receipt per cell; there is no second path-filter or handwritten queue-test
command in workflow YAML. Its shard includes the DX provenance contract,
scientific/source-build custody, and the queue suite so the hosted checkout is
exercised before test collection as well as through the product wrapper.
Queue-owned pytest commands carry `MOLT_PROOF_QUEUE_*` custody plus a canonical
`MOLT_PYTEST_CURRENT_TEST_FILE` path so the pytest bootstrap can reuse the
outer queue memory guard instead of recursively rewrapping the test process on
Windows.

If a row was deliberately parked with `--queue-only`, launch that exact row
later through the same custody boundary:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py run --run-id RUN_ID --detach
```

Do not reconstruct the original command, start a shell background process, or
submit a duplicate row unless the first row is terminal and the new row records
the rerun edge.

## Latency Discipline

Treat avoidable proof latency as a bug. Before spending a heavy slot, ask
whether the command is proving the changed invariant or merely paying for a cold
cache, a broad selector, or a stale generated file.

- Prefer exact test selectors for new invariants. A substring selector that
  misses the newly added test is false evidence; cite the precise test name or
  the precise queue run that covered it.
- Never pay a cold Cargo compile for one exact test. Use the queue-native
  `cargo` lane, batch the relevant crate shard into the same compile, and use
  `--allow-warm-single-test` only after a warmup has already made the target dir
  hot.
- Prefer a warmed canonical target/cache when it is already part of the DX
  authority and safe for the lane. If overriding `CARGO_TARGET_DIR` or another
  cache knob, record the reason in `--note`.
- Queue-owned proof runs default `MOLT_MEMORY_GUARD_POLL_SEC` to `2.0` for
  local iteration and pass that value through to `memory_guard.py`; set an
  explicit queue `--env MOLT_MEMORY_GUARD_POLL_SEC=...` override only when a
  proof genuinely needs a tighter poll. The queue validates that override as a
  positive finite number at submission time; inherited shell environment is not
  proof-row authority.
- Submit long or compile-heavy proof rows with `--detach`, then keep working or
  end the arc. Do not spend a turn tailing a queued log.
- Prefer the product front door for day-to-day queue work:
  `molt queue status`, `molt queue --help`,
  `molt queue --db logs/proof_queue/proof_queue.sqlite3 status`,
  `molt queue run --queue-size N --detach`, and
  `molt queue native-molt-run --detach path/to/probe.py`. The `molt queue`
  command delegates to this proof-queue authority; it does not own a second
  scheduler, database, log tree, process model, or contention policy.
- `--jobs`/`--limit` caps how many ready rows one `run` invocation selects.
  `--queue-size` caps active queue capacity; when detaching and no explicit
  `--jobs`/`--limit` is provided, the run limit defaults to that capacity. Both
  are cross-platform because each launched row still uses the same queue-owned
  detached runner and OS custody path; contention keys still prevent unsafe
  overlap such as two active `cargo:molt-runtime` rows.
- Queue-owned uv subprocesses default `UV_LINK_MODE=copy` unless the operator
  already set a value. This avoids noisy hardlink fallback warnings on
  cross-device caches, exFAT artifact volumes, and other valid Windows/macOS/Linux
  storage layouts without disabling cache reuse.
- When historical warning rows make audit output noisy, use
  `tools\proof_queue.py audit --errors-only` for human triage. This hides
  warning rows only from the terminal text; JSON/output payloads and the audit
  exit status still preserve real errors.
- For generators, use their timing mode when available and record the number.
  A generator check that rewrites identical files or reruns formatters on every
  output is a structural DX defect, not background noise.
- If a proof lane is already active, monitor it instead of stacking another
  Cargo/WASM proof unless the new command is independent and cheap.
- The native proof supervisor is provisioned for its run through the existing
  Cargo custody path. Source digests and a rustc version string alone do not
  prove a reusable build: configuration, wrappers, linkers, environment and
  build-script inputs also matter. Do not adopt a shared supervisor binary
  until the complete input identity is proven by the owning Cargo authority.

## TOML DSL

For multi-run submissions, use a TOML file. `note` accepts one string and
`notes` accepts a list of strings.

```toml
[[proof]]
id = "pact-field-solve-candidate"
reason = "Run Pact field_solve candidate after import transaction authority change"
resource_family = "wasm-run"
contention_key = "wasm:pact-field-solve"
scope = ["collab/pact", "wasm/run_wasm.js"]
depends_on = ["previous-run-id-or-logical-id"]
note = "Testing whether relative import canonicalization moved the failure past import_transaction."
notes = ["Expect candidate_outputs.npz or a precise next ABI primitive failure."]
edge_kind = "derives_from"
edge_note = "Narrows the previous failure to the import transaction path."
command = [
  "uv", "run", "--active", "--project", ".", "--python", "3.12",
  "python", "tmp/pact_candidate_runner.py",
]
```

Submit with:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py submit proof.toml
```

## Named Pact Witness Lanes

Use the named lane for Pact Kernel A acceptance. Do not queue ad hoc `molt
build` commands for this contract:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py pact-witness-acceptance
```

For the normal heavyweight lane, prefer:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py pact-witness-acceptance --detach
```

`pact-witness-acceptance` renders to `tools/pact_witness_acceptance.py`. That
script owns the full acceptance sequence: build `field_solve.py`, run the WASM
artifact from an isolated fixture directory, write
`tmp/pact_witness_acceptance_queue/runs/<attempt>/run/candidate_outputs.npz`,
then run `check_parity.py` against the checked Pact reference. The runner writes
`tmp/pact_witness_acceptance_queue/latest_attempt.txt` for quick navigation and
never deletes previous attempt directories, because Windows may keep linked
`.wat` or `.wasm` files open briefly after a failed run. A row whose command is
only `python -m molt build ... field_solve.py` is historical build evidence, not
Pact acceptance, and must be rerun through the named current spec after it exits.
If Node reports a static extension `Py_mod_exec` init failure, the runner emits
`run/static_extension_init_failure.json` with the matched staged manifest,
object-closure summary, source-derived capsule requirements, and source line
hints so agents do not hand-audit temp roots before reading the generated
dossier.

Before spending the heavy slot, inspect the rendered lane:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py pact-witness-acceptance --print-spec
```

Root selection is authority-driven, not directory-discovery ordered. The queue
admits exactly the versioned NumPy seal and the configured SciPy
`pact-witness` set resolved from `config/scientific_stack_versions.toml`. The
SciPy root lives under
`C:\Molt\package-seals\scipy\<version>\variants\cpython-<version>\`
`cpython-abi\wasm32-wasip1\pact_scipy_witness` on Windows (the
platform custody root, independent of `$MOLT_EXT_ROOT`) and must
contain the exact configured four-module transaction; historical per-module
roots under `tmp/` are evidence only and are never unioned or used as fallback.
Missing, extra, stale-ABI, nondeterministic, checksum-inconsistent, or
incomplete transaction manifests fail before the heavyweight acceptance lane.

## Append-Only Notes

Proof notes are append-only at the SQLite layer. Do not edit or delete notes.
If the understanding changes, append a new observation.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py note RUN_ID `
  --kind observation `
  --author codex `
  --note "R19 moved past PyInit and now traps at scipy.ndimage._nd_image isolate import."
```

Canonical note kinds are `submission`, `change`, `hypothesis`, `test`,
`observation`, `finding`, `decision`, `followup`, and `handoff`. The queue
enforces this vocabulary so status, evidence JSON, and notebook summaries stay
searchable across agents.

## Proof DAG

Proof edges are append-only at the SQLite layer and reject cycles. Use them to
make experimental lineage machine-readable instead of burying it in prose.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py link CHILD_RUN_ID `
  --parent PARENT_RUN_ID `
  --kind reruns `
  --author codex `
  --note "Replays the failed import path after the module-state fix."
```

Canonical edge kinds are `depends_on`, `derives_from`, `reruns`, `compares`,
and `supersedes`. `depends_on` is the scheduling edge; the others preserve
lineage and comparison intent for evidence review. Because queue databases are
worktree-local, non-scheduling lineage edges may name a parent run from another
worktree; `depends_on` parents must exist in the local queue so scheduling can
fail closed.

## Evidence And Notebooks

Each run records:

- command, cwd, status, return code, elapsed time
- resource family, contention key, scopes
- queue log and memory-guard summary paths
- git `HEAD`, dirty bit, and short status at submission
- append-only notes
- per-kind note counts
- append-only proof DAG parents/children, edge notes, and per-kind edge counts

Inspect machine-readable evidence with:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py evidence --run-id RUN_ID
```

Evidence includes deterministic `diagnostics` derived from queue metadata and
log tails. These are not guesses; they are first-party rules for recurring
proof failure classes such as queue policy rejection, static-linked
`Py_mod_exec` failure, unresolved native/WASM symbols, unsupported direct calls,
Pact missing-output acceptance failures, Rust compiler errors, pytest assertion
failures, external native artifact custody refusals, reachable native support
modules without source/artifact custody, reachability-driven stdlib profile
refusals, generated WASM ABI/link-import surface gaps, dependency-blocked rows,
Molt runtime invalid-object-header aborts, quiet running pytest rows with missing
current-test custody markers, non-final memory-guard summaries on terminal
rows, and memory-guard orphan cleanup.
Command-envelope, environment-override, and launch-prefix refusals share
`queue-policy-rejection`, retaining the actual rejection line rather than a
generic terminal footer. They report operator policy evidence, not an executed
product failure. Cold single-test Cargo rejection retains its more specific
diagnostic and batching guidance.
When the Pact runner emits `static_extension_init_failure.json`, the
static-link diagnostic includes that path in its `artifacts` list.

Running command diagnostics also consume bounded stdout/stderr tails from the
current execution's nonce-bound, opened-file identities. These provisional
observations include paths, byte offsets and observed sizes; they are not
terminal receipts and never authorize cancellation or another execution.
Missing or changed stream custody emits `live-command-transcript-unavailable`
instead of reading guessed paths. Complete transcript hashes remain owned by
the quiescent execution receipt.

Use `diagnose` before manual log spelunking or hand-written status notes:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py diagnose RUN_ID
```

To preserve the finding for other agents, append the deterministic diagnosis as
an immutable note and regenerate the notebook projection:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py diagnose RUN_ID --append-note
```

`status` also prints the first diagnostic for recent failed rows. If a repeated
failure only shows `unclassified-failed-proof`, add a deterministic diagnosis
rule to the owning `tools/proof_queue_pkg/diagnostic_*_rules.py` family before
that pattern becomes tribal knowledge.
`audit` also reports `audit-weak-proof-metadata` for rows that fell back to
generic resource/contention authority, have no scopes, or carry suspicious
reasons from broken shell quoting. Treat those rows as weak evidence and rerun
with the delimiter-guarded shape before citing them.
For active pytest rows, `status` prints `pytest_current=<nodeid> phase=<phase>`
when the memory-guard summary has a live marker. If the marker file is still
missing while the queue log is quiet, `diagnose` must classify the row as
`running-pytest-current-test-missing`; treat that as pre-test or collection
opacity. If the evidence includes `last_pytest_progress=...`, pytest has
started and the defect is current-test custody opacity after progress, not
startup opacity; inspect the pytest guard plugin/env wiring once, then rerun
with a focused selector only if the row does not finish. When the evidence also
names
`child_process=windows_memory_guard_child_runner`, the visible child is the
Windows child-limit runner; inspect the descendant uv/cache/startup command
once, then rerun with a focused selector instead of interrupting through Codex
stdin.
If a terminal row still has only a `running` or `child_running` memory-guard
summary with no summary return code, it must classify as
`memory-guard-summary-incomplete`; treat that row as queue-custody incomplete
evidence and rerun or fix the guard final-summary lifecycle. The diagnostic
evidence must include the row status/return code, elapsed time when known,
configured guard timeout when present, child guard identity, recorded summary
time, last log age, and last non-empty log line so the next agent can decide
from `audit`/`diagnose` output without manual tailing first. This diagnostic
dominates product-looking log matches: an incomplete guard summary must appear
first in `diagnose` output and must suppress frontier failure promotion for
that row.
If the queue itself fails before launching a proof command, it must mark the row
terminal, write the failure log, release the contention key, and classify the
row as `queue-preexecution-failure`; that row is infrastructure evidence, not
product proof.

Use `audit` for recursive queue health review before starting another long
proof tranche:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py audit
```

`audit` walks active and recent rows, diagnostics, append-only notes, DAG edges,
guard liveness, log freshness, and notebook projections. A classified product
failure is allowed to remain evidence. Queue debt is not: missing logs, queue
pre-execution failures, policy rejections, unclassified failures, dead running
guards, duplicate active contention keys, stale active logs, missing proof
notes, and missing notebook projections are surfaced as explicit audit issues.
By default the command exits non-zero for errors and reports warnings without
failing; add `--strict` when warnings should fail the pass. Human output prints
diagnostic and issue counts first, then a `frontier:` block for the latest
non-superseded classified product failures, then queue-debt issues. Rerun or
supersede edges retire older frontier failures from that block once a child row
exists, so audit points agents at the current boundary instead of replaying
stale failures. Default audit also treats superseded terminal rows as
archaeology and omits their old queue-debt issues from exit status and human
triage; the human summary prints `archaeology: superseded_terminal=N` and the
JSON payload exposes `superseded_archaeology_runs` when rows are retired this
way. Use `--all` when you intentionally want complete historical debt. Active
pytest rows that have gone quiet before writing a current-test marker surface as
`audit-running-pytest-current-test-missing` warnings, so collection/startup
opacity does not masquerade as a healthy queue. The issue wall is capped by
default; use `--max-issues 0`, `--json`, or `--output` for the full
machine-readable handoff.

For runs with notes, the queue writes a deterministic marimo `.py` notebook under
`logs/proof_queue/notebooks/RUN_ID.py` by default. Verified hosted CI redirects
that whole proof-state tree to its per-run custody root. The notebook is a generated
projection of queue evidence and log tail, not the source of truth. Do not hand
edit it; regenerate it instead:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py notebook RUN_ID
```

Use `--notebooks-root` to redirect projections for local experiments. Generated
notebooks should normally stay untracked with the rest of `logs/`.

Notebook projection is observability, not launch authority. If projection fails
during submission, run completion, `note`, `link`, or `diagnose --append-note`,
the queue must preserve the row or mutation, append a nonfatal infrastructure
failure to the run log, classify the row as `queue-infra-warning`, and continue.
Only the explicit `notebook RUN_ID` command treats notebook generation as the
requested artifact and fails directly when it cannot write that projection.

## Stall Recovery

If a queue row stalls, inspect the log and memory-guard summary first:

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py evidence --run-id RUN_ID
```

Use `prune-stale --run-id RUN_ID` for a stale row you own. The unscoped
`prune-stale` form is intentionally broad; reserve it for queue-wide cleanup
after checking active ownership. Do not kill broad process families, Codex,
Claude, renderer helpers, node-repl, shell ancestors, or ambiguous host
control-plane processes. Each pruned row prints the deterministic diagnosis
that justified pruning, compact diagnostic evidence, and the memory-guard
summary and queue log paths; treat that line as the handoff breadcrumb instead
of rerunning broad status loops.
If the queue guard process is still alive but the nested memory-guard child in
the running summary is dead and the log is stale, `prune-stale` must still mark
the row stale with `running-proof-child-missing`; do not keep that row active
waiting for a child that custody already proved gone.
Active queue-owned runners apply the same stale-running diagnostics while they
wait. `running-proof-child-missing` and
`running-proof-guard-timeout-expired` are self-terminalizing runner signals;
the latter means the guard's own declared timeout plus finalization grace has
already elapsed without a terminal summary. Persist it with
`diagnose RUN_ID --append-note`, then use targeted
`prune-stale --run-id RUN_ID`; never kill or restart from PID evidence alone.
`running-proof-launch-summary-stale` means the guard summary has not yet
advanced far enough to prove nested custody, so the runner must keep waiting
for the guard process it launched. Use targeted pruning after an ownership
check when a launch-summary-only row truly needs manual cleanup.

```powershell
uv run --active --project . --python 3.12 python tools\proof_queue.py prune-stale --run-id RUN_ID
uv run --active --project . --python 3.12 python tools\proof_queue.py prune-stale
```

When citing proof, cite the run ID plus the log or evidence path. Treat
uncertain, stale, or dirty-run evidence as partial until the current tree proves
the claim.
