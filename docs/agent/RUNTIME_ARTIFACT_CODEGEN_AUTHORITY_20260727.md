# Runtime artifact codegen authority

## Aperture

The native `molt-runtime` producer asked Cargo for `native-static-libs`, but the
manifest declared `staticlib`, `rlib`, and `cdylib`. One load-bearing rustc
therefore emitted every declared crate type even though the native linker
consumed only the static archive. The same additive mistake existed in the
per-artifact WASM commands: a trailing rustc-level `--crate-type` did not replace
Cargo's manifest plan.

The hot path is LLVM code generation and final artifact emission for the
288k-line runtime fan-in. Its source traversal remains O(runtime source and
monomorphization closure); the structural win is deleting unused crate-type
emission and link work from each final-artifact producer, rather than changing
optimization level or raising the memory limit.

## Authority

`src/molt/cli/runtime_artifact_selection.py` is the one typed producer
authority. It emits Cargo's pre-separator `--crate-type <comma-separated-set>`
option and rejects selection after Cargo's `--` separator.

| producer | exact selected crate types |
|---|---|
| Rust dependency/default manifest | `rlib` |
| native runtime link archive | `staticlib` |
| WASM reloc link input | `staticlib` |
| WASM shared runtime | `cdylib` |
| combined split-runtime build | `staticlib,cdylib` |

The selected artifact set is folded into the existing runtime fingerprint
metadata. Native-link source attestations, session verification, target
fingerprints, and shared WASM caches therefore cannot reuse an artifact whose
producer selected a different crate-type set. Publication and build identity
remain owned by their existing authorities; this module owns only producer
artifact selection.

## Runtime identity consolidation (2026-09-05)

### Build storage and profile inheritance

`Cargo.toml` owns profile settings through Cargo inheritance, including package
overrides. `dev-fast` inherits the backend/runtime debug and optimization policy
from `dev`; `release-fast` changes only its iteration-specific fields. Shipping
policy flows from `release-output` into `release-size`, whose hot-crate size
overrides also serve `wasm-release`. `dev-release` retains the release policy
with symbols. Do not copy inherited fields into child profiles: test effective
policy and preserve target-specific artifact selection instead.

Build storage is an optimization target alongside wall clock and peak memory.
Measure prerequisite footprint (toolchains, SDKs and dependencies), additional
peak working storage for cold and warm builds, final retained outputs, reusable
cache, and retired generations separately. Attribute measurements to the source
revision, target, profile and cache state. A post-failure directory size is only
partial retained output, not a measured successful-build peak. Logical file sizes
can double-count hard links; filesystem free-space deltas can include unrelated
host activity.
Report those qualifications with before/after measurements. The capacity floor
in `molt.disk_capacity` is an admission guardrail, never a footprint target or
evidence that a build fits. Reduce unnecessary artifact production and duplicate
retention before changing that floor; keep useful diagnostics and valid warm
reuse explicit in the tradeoff.

`runtime_identity_schema.py` owns exact v3 compile/family/member receipts;
`runtime_build_identity.py` captures inputs and projects those receipts. The
immutable resolved Cargo plan owns effective configuration, selected tools,
resource custody, target and profile policy. Capture and execution consume the
same command and environment. A failed wrapper is not retried under a different
unattested toolchain.

Runtime flag projection captures the ordered logical roots and their text forms
once per identity operation. Native, standalone WASM ABI, and shared/relocatable
runtime members use this same projection; export-only tokens perform no filesystem
resolution. Absolute operands still resolve and must stay within an admitted root,
response arguments retain the captured Cargo plan's custody, and command-specific
source-first precedence is explicit. The next operation captures roots afresh;
there is no process-wide path cache or relaxed path admission.

Cargo `[env]` values retain their config-relative source origins, ambient/force
precedence, and Windows case-insensitive key semantics. Selected tools, wrappers,
Rust sysroots, codegen backends, extern files, and library search directories have
live byte-generation custody. The WASM input-capture hook observes the resolved
environment, tools, and ordered target library roots once; it does not rediscover
ambient configuration. Inherited profile overrides use the same profile ancestry
as debug policy. Environment attribution discovers the complete profile namespace
from the captured manifest, config files and CLI configuration before excluding
unselected siblings. Exact supported controls take precedence over profile-name
prefixes: a profile named `release-build-override` cannot hide release build-script
controls. Hyphen/underscore aliases share Cargo's environment spelling. Unknown
controls belonging to selected profiles still fail closed.

Cargo target predicates are parsed once by `cargo_target_cfg.py` against a shared
typed rustc target-metadata query, never guessed from the host platform. The
query reproduces Cargo's stdin/crate-name/crate-type/print envelope, including
its marker-delimited output and unsupported-crate-type diagnostics. Implicit
host queries omit `--target`; explicit target queries retain it.
Host, cfg and resource probes share Cargo's nested wrapper order
(`RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, rustc), with executable mutation
fences around the whole chain and pre-probe custody retained through execution.
The user-specific `proc_macro` marker is omitted from target facts, as in Cargo.
File and CLI StringList arrays merge before target/build config environment
flags are appended. Whole-vector `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS` overrides
remain separate. Exact-target flags precede matching cfg flags in Cargo's
deterministic key order; an empty target vector falls back to build flags.
Exact-target linker selection wins over cfg selection; multiple matching
cfg linkers fail before execution. Flags/cfg mutual dependencies must converge
within Cargo's two-pass boundary; nonconvergence receives no runtime identity.
The final transformed flags are pinned in the execution environment, and linker
selection observes their cfg facts. This replaces the blanket cfg-table refusal.
The same per-plan metadata results determine target resource roots. Both
dependency and final-crate effective flag lanes are queried through wrappers,
so wrapper-selected sysroots cannot disagree with the admitted cfg envelope.
Unwrapped installed-compiler metadata separately preserves driver/codegen
resource custody; it is never replaced by a wrapper's virtual target sysroot.
Reference: [Cargo configuration](https://doc.rust-lang.org/cargo/reference/config.html#buildrustflags).

Final-link responses have separate live custody and member projections, so export
changes do not invalidate the shared static-library compile identity. Their
admitted language is the generated runtime response grammar in `wasm_link_args.py`:
non-resource switches, explicit runtime exports, and numeric table bases. Nested
response files, scripts, external archives/search paths, and unknown response
arguments fail with explicit unsupported-resource diagnostics before execution;
this is not a claim of arbitrary external linker-dialect support. Rust resource
selectors outside response files use the typed Cargo plan's file/tree closure.

Native linkage uses manifest v5 and portable dependency custody v2. Bundle v3
derives its target from the runtime receipt; extraction never guesses target
semantics from the receiving host. Producer directories are not link inputs.
WASM generation v3 and expected-pair v2 bind both members to one family, retain
stable file identities through final-link snapshotting, and share the resolved
linker/archive selection with build capture. Export-only member changes retain
the static-library compile identity. The deleted per-member Cargo builder and
metadata-only runtime fingerprint are not fallback authorities.

Live family resolution always captures the selected toolchain. Portable toolchain
manifests are projections of that capture, never substituted as live inputs.
The pair and final-preflight consumers capture once, then publish that projection;
`runtime_family_identity` measures the complete pre/post capture without counting
an independent toolchain-provision pass.
Cargo tool and wrapper records project the already-captured executable custody;
building the family receipt does not hash those same executables again.
Native-executable admission remains part of that capture, including distinct
dependency and final-crate linker roles. Generic resource hashing does not admit
a script linker without its interpreter closure.

WASM link inputs are captured only after Cargo configuration, environment, and
Rust target-library selection. Both members retain that same capture; final
linking never rediscovers an ambient Rust target. Linker custody preserves the
invoked `wasm-ld` alias while attesting its physical content. Captured executable
search includes PATH, Windows PATHEXT/key casing/current-directory policy, and
managed roots; cache lookup uses those inputs rather than ambient equivalents.
Export response files are materialized once per resolved producer, not separately
for each preliminary member spec.

The resolved Cargo plan also owns C/C++ flag tokenization, ordered include/library
search paths, and their stable resource contents. Identity consumes its typed
projection rather than independently parsing cc-rs flags. Relative search paths,
GCC empty (working-directory) search entries, and unparsed compiler response or
forwarding forms fail with explicit custody diagnostics; the current local Cargo
source closure does not establish every registry build-script working directory.
Explicit search custody does not establish a complete implicit system-SDK or
compiler-generated include closure, which remains an unverified support frontier.
Compiler sysroot/prefix-relative include and library operands likewise reject
when their effective base has no custody; an option separator is not permission
to reinterpret such an operand as a host-absolute path.

Runtime JSON readers share bounded stable-file exact decoding (16 MiB metadata
ceiling). Artifact byte/archive receipts reject bool/float substitutions for
integer counts through one validator. Archive parsing and raw byte hashing use
one stable direct-file handle, not a metadata-keyed digest cache; same-size,
preserved-mtime rewrites are re-read on every admission. Archive name metadata
shares the 16 MiB bound. Native custody and bundles admit only
regular USTAR records before tar extension decoding; bundle closure is bounded
before member retention and staged identities survive to publication. Every
operational WASM builder failure, including the standalone CPython ABI provider,
uses the same bounded evidence model; JSON mode retains subprocess output,
timeouts, identity drift, and evidence-write errors.
Native build and manifest-refresh timeouts retain partial streams as well.
Native fingerprint publication and refresh fail admission on write errors;
failure-evidence persistence errors remain visible even without attached state.

Backend probe receipts, object-cache variants and daemon selection likewise use
the shared stable executable-content primitive. Probe publication fences the
executable generation around execution. The existing compile-local fingerprint
passed into the Rust TIR cache includes executable content as well as the source
projection; a metadata-preserving rewrite cannot select the old namespace.
Source fingerprints remain rebuild selectors, not executable-content custody.
Feature aliases and canonical backend hydration require the same source-and-byte
receipt. Raw Cargo outputs and newer timestamps cannot attest provenance; Cargo
must establish it before publication. Feature lanes share the canonical output's
publication lock, and probe/receipt write failures remain typed build failures.

The canonical pure proof command is `python.unit.runtime-artifacts` in
`tools/proof_plan.toml`; tool-search primitives belong to
`python.unit.python-custody`. Generated projections are not separate checklists.

The earlier timings below are historical receipts, not measurements of this
consolidation. Initial verification was serialized Python-only under temporary
2 GiB process / 3 GiB tree limits. The operator lifted those limits on September
5; normal resource guards remain, with one heavyweight compiler proof lane
alongside disjoint integration work. A compiled native/WASM matrix cell,
benchmark improvement, main landing, or donor retirement still requires its
own current-source receipt.

## Recovered baseline evidence

All paths are under the canonical `C:\Molt` root.

| shape | wall | peak process RSS | peak tree RSS | result |
|---|---:|---:|---:|---|
| manifest `staticlib+rlib+cdylib` | 657.593 s | 11,518,263,296 B | 11,721,920,512 B | success |
| trailing rustc `--crate-type=staticlib` | 706.797 s | 11,585,171,456 B | 11,741,495,296 B | success; additive selector falsified |
| Cargo-level `--crate-type staticlib` attempt | 566.234 s | 3,881,418,752 B | 4,087,083,008 B | interrupted (`0x40010004`); no completed archive, so not a success claim |
| landed authority, Cargo-level `staticlib` | 1,201.031 s | 5,493,481,472 B | 5,642,506,240 B | guarded timeout (rc 124); no completed archive |

Metric files:

- `tmp/runtime-artifact-codegen-profile/baseline-all.metrics.json`
- `tmp/runtime-artifact-codegen-profile/static-only.metrics.json`
- `tmp/runtime-artifact-codegen-profile/static-selected.metrics.json`
- `tmp/runtime-artifact-codegen-proof-20260727/native-cold.metrics.json`

The CI failure that opened the aperture is
`tmp/ci-30212200219-llvm/expanded/target/.molt_state/build_failures/native-runtime-cargo-24240-12d7f43b20194de6986f8f4dca6fe2ff.json`:
LLVM rustc was terminated at 3,986,206,720 B process RSS and 4,005,445,632 B
tree RSS. The interrupted local selector attempt is evidence of the right
memory-pressure direction, not proof of completion. The final guarded cold run
cut peak process RSS by about 52% and tree RSS by about 52% versus the successful
manifest-wide baseline, proving that the producer authority deletes substantial
unused codegen. It still exceeded the CI envelope by roughly 1.5 GB and did not
finish inside 20 minutes. Exact artifact selection is therefore a valid
structural landing, but it does **not** close the CI OOM.

The next distinct aperture was the shipping codegen policy in root `Cargo.toml`:
`release-output` combined fat LTO, one codegen unit, and inherited `debug = 1`.
The matrix below closes that authority rather than adding a CI-only alias or
retry.

## Shipping codegen policy matrix

Every native row used the exact full-feature staticlib producer, a fresh target
directory, one guarded Cargo process tree, and the same source revision. The
fat/1 timeout peak is a lower bound because rustc had not completed.

| release-output policy | wall | process RSS | tree RSS | Job commit | archive | result |
|---|---:|---:|---:|---:|---:|---|
| fat / 1 / debug=1 | 1,201.031 s | 5,493,481,472 B | 5,642,506,240 B | 5,674,582,016 B | none | timeout |
| thin / 4 / debug=1 | 316.391 s | 6,907,158,528 B | 7,075,172,352 B | 7,240,884,224 B | 156,364,450 B | pass |
| thin / 16 / debug=1 | 198.875 s | 4,457,263,104 B | 4,622,839,808 B | 4,674,871,296 B | 160,247,186 B | pass |
| thin / 32 / debug=1 | 181.219 s | 4,286,345,216 B | 4,451,274,752 B | 4,491,722,752 B | 162,616,328 B | pass |
| **thin / 16 / debug=0** | **168.391 s** | **2,810,617,856 B** | **2,977,423,360 B** | **3,118,239,744 B** | **63,665,544 B** | **pass** |

Evidence:

- `tmp/runtime-codegen-policy-matrix-20260728/native-thin4.metrics.json`
- `tmp/runtime-codegen-policy-matrix-20260728/native-thin16.metrics.json`
- `tmp/runtime-codegen-policy-matrix-20260728/native-thin32.metrics.json`
- `tmp/runtime-codegen-policy-matrix-20260728/native-thin16-nodebug.metrics.json`

The selected shipping authority is ThinLTO, 16 codegen units, and no debug
metadata. Relative to thin/16 with inherited debug metadata, removing debug cut
wall by 15.3%, process RSS by 36.9%, tree RSS by 35.6%, and the static archive by
60.3%. It is 1.18 GB below the earlier Linux CI rustc failure frontier. CGU32's
small wall/RSS improvement under debug=1 did not justify its extra member and
archive fragmentation once the actual debug-metadata cause was removed.

`release-output`, `release-size`, and `wasm-release` now share that codegen and
debug policy. Profile scope owns LTO/codegen units; package overrides own only
the hot-crate opt level. `dev-release` remains the explicit symbol-bearing
profile. Invalid WASM output fails closed under the one primary profile; the old
isolated-target and alternate-profile retry chain is deleted. The combined
`staticlib,cdylib` producer is likewise mandatory for split-runtime `both` builds:
its two legacy environment kill switches and automatic sequential dual-compile
retry are deleted. Atomic generation custody remains authoritative for every
split-runtime build, including freestanding consumers that select only the reloc
member downstream.

## Exact split-runtime pair proof

The final `both` path was measured with an isolated Cargo target and cache after
the publication and codegen authorities were combined. It performed one Cargo
compile, reused both declared crate-type outputs from that compile, transformed
and published one immutable shared/reloc generation, and created no fixed-name
artifact authority.

| measurement | wall | process RSS | tree RSS | Job commit | result |
|---|---:|---:|---:|---:|---|
| guarded cold pair | 402.906 s | 3,214,643,200 B | 3,475,525,632 B | 3,411,423,232 B | pass |
| instrumented Cargo + publication phases | 169.905 s | - | - | - | 1 compile, 2 target reuses |
| guarded warm pair before identity optimization | 8.640 s | 98,983,936 B | 170,430,464 B | 189,558,784 B | pass |
| guarded first local-generation reconciliation after identity optimization | 20.593 s | 121,004,032 B | 189,333,504 B | 198,057,984 B | pass; 0 Cargo compiles |
| guarded steady-state immutable-generation read | 7.625 s | 113,065,984 B | 181,796,864 B | 190,763,008 B | pass; no identity scan or Cargo compile |

The published shared member is 30,949,871 B, the reloc member is 44,842,469 B,
and their pair digest is
`3d1aa8c79761bdaa477c333e79d79c577bd5cb8f72078f1b12db57cb00742c23`.
The first post-optimization reconciliation spent 9.618 s on the exact 17,983-file
toolchain identity and 1.146 s on the 694-file source identity. It selected the
existing immutable generation without compilation or publication. The next
steady-state read consumed that immutable manifest directly in 7.625 s without
rescanning identity or invoking Cargo.

Evidence:

- `C:\Molt\tmp\runtime-final-combined-20260728\pair-build.metrics.json`
- `C:\Molt\tmp\runtime-final-combined-20260728\pair-warm.metrics.json`
- `C:\Molt\tmp\runtime-final-combined-20260728\pair-final-warm.metrics.json`
- `C:\Molt\tmp\runtime-final-combined-20260728\pair-final-steady.metrics.json`
- `C:\Molt\tmp\runtime-final-combined-20260728\diagnostics-final.jsonl`

## Exact content-identity throughput

The cold pair profile exposed an uninstrumented identity gap: the exact WASI
sysroot closure was enumerated and hashed sequentially before Cargo, then
repeated after Cargo to reject build-time input mutation. The second scan is a
correctness invariant, so it remains exact and uncached.

The canonical identity walker now uses fail-closed `scandir` enumeration,
resource-adaptive bounded scheduling, reusable 1 MiB per-worker buffers, and
deterministic result assembly. Snapshot and post-read checks compare file
identity, size, mode, mtime, and the platform content-change time; aliases,
mutation, and I/O errors fail closed. Windows NTFS ChangeTime moved out of the
LLVM-specific implementation into the shared file-hashing authority consumed by
runtime identity, LLVM attestation, and LLVM bootstrap. Pre/post source and
toolchain phase walls and selected worker counts are emitted in build
diagnostics but are excluded from the content identity.

An interleaved `old/new/new/old/new/old` benchmark used separate child processes
over the same four WASI roots. All six runs produced the same digest over 17,983
files and 249,161,774 B. The unchanged authority median was 55.443 s; the final
authority median was 7.945 s, a 6.98x speedup with 24 resource-selected workers.
The final profile attributed 0.192 s to enumeration/aggregation, 2.237 s to
parallel snapshot, and 5.427 s to exact hashing and post-read validation.

Evidence:

- `C:\Molt\logs\agents\runtime_identity_parallel_hash\sysroot_hash_benchmark_20260728.json`

## Proof contract

The landing proof must show:

1. command-shape and manifest tests reject manifest-wide or trailing additive
   crate-type selection;
2. native, reloc, shared, and combined producers request only their typed set;
3. Cargo reports exactly the artifacts each producer expects and cache identity
   changes with the selected set;
4. cold and warm guarded native/WASM builds record wall time, process/tree RSS,
   artifact names and sizes;
5. native and WASM runtime execution/determinism checks remain unchanged; and
6. CI-shaped scheduling does not overlap the load-bearing runtime rustc with a
   broad truth lane merely to trade one OOM for another; and
7. split-runtime `both` fails closed after a combined-producer/finalization
   failure and never launches a per-artifact Cargo retry.
