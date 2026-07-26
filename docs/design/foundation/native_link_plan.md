# Native link planning

`src/molt/cli/native_link_plan.py` is the canonical native link policy
authority. A link is planned once as an immutable tuple of:

- resolved target OS, architecture, and object format;
- selected linker and its demonstrated capabilities;
- optimization policy (dead stripping, function identity, relocations,
  post-link stripping, and BOLT intent); and
- the exact command used by the fingerprint and executor.

Main executables and source-built native extensions consume the same dead-strip
and function-identity flags. User and upstream link arguments are inputs to the
plan, not permission to weaken its correctness invariants.

## Function identity

Molt runtime behavior can observe function-pointer identity. Until code objects
carry a separate stable identity independent of machine addresses, identical
code folding is disabled explicitly where the selected linker supports it:

| Format | Policy |
| --- | --- |
| COFF | `/OPT:NOICF` |
| Mach-O | `-no_deduplicate` |
| ELF with lld or mold | `--icf=none` |
| ELF system linker | no ICF flag; the default linker policy is no folding |

## BOLT

BOLT is a release-only Linux ELF post-link stage on x86_64 and aarch64. The
ordinary linker emits relocations and retains symbols. The pipeline merges all
PID-scoped training fragments, strips the optimized candidate, validates that
final candidate, and only then atomically publishes it. A failed finalization
therefore preserves the previously published artifact. Each BOLT run starts
from a fresh ordinary link because the link-input fingerprint does not describe
workload profile data.

Release stripping is target-aware: exact-host targets may use the resolved
native strip tool, while a foreign OS or architecture requires `llvm-strip`.
Missing or failed target stripping fails the release build instead of silently
publishing an artifact outside the plan.

Unsupported targets, missing tools, invalid optimized images, failed training,
and failed stripping are diagnostics, never silent skips. Custom training
commands must use `{binary}` to name the instrumented image.

The command is executed once. A selected linker failure or invalid output fails
the build; there is no post-failure retry through a different linker.

Cargo-native dependency discovery has one artifact-bound authority:
`<runtime>.native-link-deps.json`. The exact successful runtime command is
`cargo rustc ... --message-format=json-render-diagnostics -- --print
native-static-libs`. Rustc's exact `native-static-libs` diagnostic—captured from
Cargo's structured compiler message when present, or its strict stderr note—is
the sole authority for the final native-library argument sequence: its order
and duplicates are persisted and replayed verbatim because static archive order
is semantic. Cargo `build-script-executed` events are not treated as a cross-script
ordering authority; parallel script completion can reorder them. Their full
package IDs, `out_dir`, `linked_paths`, and per-script `linked_libs` remain
canonicalized non-semantic provenance used to validate custody and resolve the
files named by rustc.

The manifest binds the runtime through the shared semantic static-archive
identity used by runtime publication, hydration, native link fingerprints, and
link benchmarks. That identity hashes the ordered linkable member names and
contents for COFF/GNU/BSD archives. It excludes container-derived symbol and
long-name tables, non-semantic ar header metadata, and only the structurally
recognized rustc CGU discriminator positions of local `molt_*` members. Any
member content, name, order, addition, or removal still invalidates the
manifest and every consuming cache.

Rustc prints linker tokens, while Molt invokes a compiler driver. On COFF, bare
`.lib` names and `/defaultlib` options are forwarded one-for-one with Clang's
`-Wl,` transport so the driver cannot misclassify them as filesystem inputs;
the payload spelling, order, and duplicates remain unchanged. There is no
separate hard-coded Windows system-library list.

The atomically published, strictly UTF-8/JSON manifest records the Cargo
profile/target, runtime archive size/SHA-256, and the complete
source/config/toolchain fingerprint. Its digest fields are validated as lowercase
SHA-256 values rather than arbitrary strings. Runtime readiness carries that
verified fingerprint into native plan construction; every production link read
requires the same fingerprint. The invoking workspace must exist while its
fingerprint is computed and consumed, but its absolute path is deliberately not
persisted as semantic identity: byte-equivalent sibling worktrees and hydrated
archives therefore share one content-addressed sidecar without last-writer path
thrash. Plan construction
also rejects missing, corrupt, wrong-profile, wrong-target, archive-mismatched,
foreign-source, or stale-path manifests. Every recorded `out_dir` and
native/framework search directory must still exist. This prevents a hydrated
artifact from retaining link-input custody in a deleted worktree. It
never scans `target/<profile>/build/*/output`, never chooses a newest directory,
and never matches provenance by crate name.

Repeated plan construction memoizes the expensive archive digest by robust file
identity (resolved path, size, modification/change times, device, and inode) in a
bounded process-local cache. The reader re-stats around the lookup; replacement
or mutation changes that identity, forces a fresh SHA-256, and still fails against
the artifact-bound digest. This removes repeated `O(runtime archive bytes)` reads
from warm plans without weakening content custody.

Build-script search directories are treated as an unordered custody set, not a
surrogate link plan. Each rustc library argument is resolved against that set:
a unique package-owned match receives an explicit adjacent search directory (or
exact COFF path), an ambiguous match fails closed, and an unmatched argument
remains a toolchain/system library. Framework lookup follows the same rule.
This preserves rustc's target-specific lowering—including whole-archive,
as-needed, verbatim, framework, and platform library forms—without reimplementing
Cargo modifier semantics. Fingerprint reuse requires the matching manifest.
Canonical hydration copies a sidecar only when its content-addressed source
identity matches the invoking workspace fingerprint; otherwise it refreshes
through the same exact no-op-capable
Cargo rustc command and accepts the result only when the selected archive is
byte-identical. Failure is fatal.

The former archive-member parser and crate-name scanner were deleted when the
Cargo JSON manifest became the sole dependency authority. Keeping them would
leave an unused second provenance lane and optimize code no production link
consumes.

Native driver, fast linker, COFF librarian, strip, and inspection tools share
one resolver: an explicit `CC`/`MOLT_*` override is intentional policy;
otherwise the project-managed pinned LLVM family is preferred as a unit, with
PATH only as the final fallback. Development worktrees also search their Git
common checkout's managed target, preventing sibling worktrees from drifting to
a different system LLVM. Host Windows plans therefore select the pinned
`clang`/`lld-link`/`llvm-lib` siblings, just as host Linux plans select managed
`mold` or `ld.lld` when available. LLVM linker selection has four disjoint
entrypoint roles: `wasm-ld`, `ld.lld`, `ld64.lld`, and `lld-link`. The shared
resolver preserves those lexical entrypoints even when they are symlinks or
hardlinks to the same physical `lld` driver; generic `lld` and sibling roles
cannot satisfy a role-specific contract. The immutable plan records the driver
and linker capability it will execute instead of depending on an undocumented
default. macOS retains its platform linker unless the operator makes an explicit
supported `ld64.lld` selection. The link cache and benchmark verify the effective
executable under the same exact-role authority. The hot incremental cache keys
the lexical path and filesystem mutation identity; the benchmark additionally
uses the driver's dry-run trace and fingerprints every tool's bytes and version.
`-print-prog-name` alone is not trusted because it can disagree with the
executable selected for a complete link command.

The shared candidate resolver memoizes its complete deterministic search ladder
by tool names, explicit commands, sibling and target roots, PATH/PATHEXT, Rust
toolchain environment, current directory, checkout identity, and the portable
identity of every searched directory. Directory creation or mutation selects a
new snapshot automatically, while selected paths are existence-checked on every
hit as a coarse-timestamp backstop. Tool bytes and versions remain separately
fingerprinted, so this removes repeated filesystem discovery without weakening
executable custody.

Archive scheduling is object-format policy, not a shared fallback. ELF uses one
explicit archive group, COFF presents the runtime archive once, and Mach-O alone
retains the measured second archive pass required by ld64's order-sensitive
extraction. COFF also carries the linker-native `/Brepro` flag through the Clang
driver so PE/COFF timestamps and build metadata are deterministic. The policy is
constructed once in `native_link_plan.py`; the command builder does not grow a
second target classifier.

Every native rebuild links to a private candidate. Ordinary and BOLT builds
share one finalizer that applies target stripping when planned, validates the
final candidate, and atomically publishes it. A failed link, strip, or
validation therefore cannot overwrite or delete a previously valid output.

Fail-closed source guards keep the identity flag literals in this authority,
require executable and source-extension consumers to use the shared plan/policy,
and reject the return of post-failure linker retry lanes.

## Performance and measurement authority

`tools/native_link_benchmark.py` is the canonical native-link profiler. It
imports the production plan, command retargeter, memory guard, and candidate
finalizer; it does not reconstruct flags, stripping, validation, or publication.
The measured hot path is:

```
plan -> link candidate -> optional BOLT -> strip -> validate -> atomic publish
```

Its structural cost is `O(input bytes + symbols + sections + relocations +
output bytes)`. The report records:

- cold/warm plan wall time, net surviving Python allocation count/bytes, and
  peak traced allocation;
- cold-first, warm, and relink child wall time plus guard/orchestration wall;
  portable child user/system CPU; peak linker-process and complete process-tree
  RSS; and Windows Job peak commit charge for the whole linker tree;
- ordered input count/bytes/content identities, output size/hash, symbol,
  section, and relocation counts, and pre/post-strip byte delta;
- resolved driver, selected linker, strip, inspection, and BOLT tool versions
  and executable hashes;
- BOLT instrument, train, merge, and optimize phase times plus training fragment
  count/bytes.

Schema version 2 requires every linker child executed on a Windows host to
report whole-Job peak commit charge from the shared memory guard. Missing Job
telemetry fails before publication, and stored Windows-host reports without it
are rejected during validation. Other hosts retain the same portable tree-RSS
contract and record Job commit as unavailable; the profiler never creates a
second process owner or memory sampler.

Cold-first means the first forced link to a fresh private candidate. Warm means
an unchanged repeated plan/input link. Relink means the same plan with an
existing published artifact, exercising candidate replacement and finalization.
The tool does not claim to flush kernel filesystem caches; that would require
privileged, machine-global mutation and would make ordinary developer runs less
safe and less reproducible.

Every fingerprint has an explicit schema version. Plan, ordered input, resolved
toolchain, host/environment, and measurement-authority identities remain
separate so drift diagnostics name the changed authority. The latter
content-addresses the benchmark, process/memory guard, quiescence probe, and tool
identity resolver so a profiler implementation change cannot silently reuse an
older baseline. Response files are content-addressed first-class inputs.
Comparison is rejected before execution if any identity differs.

Warm comparisons require certified whole-host quiescence before and after both
runs, zero detected competing Cargo/rustc/backend/wasmtime processes, at least
five samples, and relative median absolute deviation at or below 5% on both
reports before they are marked attestable. The report retains non-quiescent and
bimodal samples as evidence; it never silently promotes them.
No optimization may be claimed from a descriptive/noisy comparison. A linker
choice, flag, response-file shape, tool upgrade, input change, target/profile
policy, or host change requires a new baseline rather than being averaged into
an old one.

The matrix is explicit over Linux/macOS/Windows and x86_64/aarch64. A report
proves only its recorded cell; cross-target plan construction is not evidence of
linker execution on that target. BOLT remains gated to host-executable Linux ELF
x86_64/aarch64 cells.

Example after a normal Molt build has produced the three canonical inputs:

```text
python tools/native_link_benchmark.py \
  --object <program.o> --stub <main_stub.c> --runtime <runtime.a> \
  --output <bench-program> --profile release --warm-runs 7 \
  --json-out <report.json>
```

Use `--compare <baseline.json>` only for an exact-identity before/after run.
Use `--plan-only` when the changed authority is plan construction itself; this
keeps the attestation window short and does not manufacture unrelated linker
samples while another metric is under test. Full and plan-only reports have
different comparison identities and cannot be mixed.
