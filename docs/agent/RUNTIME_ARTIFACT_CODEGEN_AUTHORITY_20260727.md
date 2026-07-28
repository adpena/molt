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

The next distinct aperture is the shipping codegen policy in root `Cargo.toml`:
`release-output` combines fat LTO with one codegen unit (also inherited by the
runtime package). That single LLVM merge remains larger than the CI process
budget even after unused crate types are removed. Changing it requires a
separate measured native+WASM profile matrix; lowering optimization or adding a
CI-only fallback here would be an unproven workaround.

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
   broad truth lane merely to trade one OOM for another.
