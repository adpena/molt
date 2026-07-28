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
