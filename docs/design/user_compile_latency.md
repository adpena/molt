# User compile latency: the sealed witness

Status: measured; the WASM no-op rebuild fixed, 2026-10-29. Owner: build
speed. The record contract stays in [build_early_cutoff.md](build_early_cutoff.md);
this note holds the measurements, the attribution and the WASM fix. Family Q
of [findings_prior_art.md](findings_prior_art.md#q-user-compile-and-edit-latency)
holds the prior art.

## Workload and method

The workload is the sealed package/data/argv witness of the 2026-10-29
handoff: `program.py` imports `fixture_package`, reads `data/input.txt` and
prints `sealed:7` (seal `c0a39f06…`, 41 compiled modules). Builds use the perf
harness's environment and argv (`--build-profile release --cache --target wasm
--linked --require-linked --python-version 3.12 --backend cranelift
--capabilities fs.read`; native selects `release-fast`) on a byte-identical
working copy with a fixed output directory, plus `--verbose`. *JSON* keeps the
harness's `--json`; *plain* drops it, as a user's `molt build` does. Host:
macOS arm64, 18 cores, CPython 3.12.15, external build volume shared with
other lanes, load 7-11. A cell gives the median, then min-max and n. No timed
build sets `MOLT_BUILD_DIAGNOSTICS`. Before/after rows come from matched blocks
that swap the changed compiler sources between the base commit and the lane,
warm up twice and then sample, in alternating order.

## Measured

| Case | WASM before | WASM after | Native before | Native after |
| --- | --- | --- | --- | --- |
| (a) compiler provisioning, cold Cargo target | backend 135.1 s, runtime 99.2 s, facts tool 17.6 s (1 each) | unchanged | backend inside a 233 s first build (1) | unchanged |
| (b) cold compile, fresh `MOLT_CACHE`, JSON | 64.3 s (63.4-65.2, 2) | 67.8 s (60.9-74.8, 2) | 33.5 s (28.4-38.6, 2) | 23.8 s (22.6-25.0, 2) |
| (c) unchanged rebuild, plain | 8.55 s (5.83-29.52, 16) | 0.66 s (0.53-8.09, 16); 0.63 s (0.57-0.93, 7) after the store fix | 0.38 s (0.33-4.88, 16) | 0.37 s (0.32-4.66, 16) |
| (c) unchanged rebuild, JSON | 6.0 s (5.9-16.3, 5) | not eligible | 1.25 s (1.10-4.29, 5) | unchanged path |
| (d) one-statement edit, plain | 45.3 s (35.0-69.9, 8) | 39.2 s (32.4-72.2, 8) | 14.0 s (7.2-31.1, 8) | 9.7 s (8.5-18.5, 8) |
| (d) first unchanged build after an edit, plain | a full no-op rebuild, as (c) | 0.59 s, 0.57 s | record hit | record hit |
| (e) link inside an edit build | 11.9 s link child of an 18.2 s sampled build; `wasm-opt -Oz` 10.9-17.8 s | unchanged | 0.7 s (sampled) | unchanged |
| (f) program start, `sealed:7` | 191 ms via Node (182-215, 10) | same bytes; 240 ms at higher load | 18.8 ms (18.6-320, 10) | same bytes; 29.7 ms at higher load |

The cold rows are noise-bound (an unmatched earlier pair gave 55.6-61.9 s and
23.3-23.8 s); a JSON request takes the same path before and after.
CPython runs the program in 12 ms. An installed Molt pays no (a): it admits
the shipped compiler (`installed_compiler_admission`) and never runs Cargo.
Fresh-path builds under the base and the lane sources produced byte-identical
products: WASM `output.wasm` `987a22ed…`, `output_linked.wasm` `bb37f572…`,
`manifest.json` `ad4b1af4…`, optimizer attestation `3940a72b…`; native
`program_molt` `08525e87…`. The sealed replay passes output parity after the
fix on both targets (compile 61.8 s WASM and 16.5 s native into the harness's
fresh output directory, without Cargo).

## Findings

1. **The backend binary is content-keyed, not session-keyed.** The host-tool
   identity of a feature set is the same under the lane target,
   `sessions/perfscore` and a fresh session (wasm-backend `5c0ff799…`,
   native-backend `83c3b882…`), and the product store serves it to every
   target. A build rebuilds the backend only when its crate source closure,
   `Cargo.lock` dependencies, toolchain or Cargo-plan inputs, profile or
   feature set change: once per feature set per host. The handoff's 171 s came
   from a lane that had changed backend inputs (`wasm_abi_manifest.toml`) and
   built cold in a fresh session target. No user flow pays it.
2. **The launcher can change the compiler identity.** Apple's
   `/usr/bin/python3` is an `xcrun` shim that exports `SDKROOT`,
   `CPATH=/usr/local/include` and `LIBRARY_PATH=/usr/local/lib` into every
   child. They are declared C inputs of the backend's build scripts, so a build
   started under it selects another backend identity (Cargo build directory
   `a66d988e…` instead of `5d9e9970…`). It cost a 135 s and a 233 s backend
   rebuild in this lane and changed the native binary bytes (another SDK). The
   invalidation is correct; start Molt tooling with the managed interpreter.
3. **The largest recurring cost on unchanged inputs was the WASM no-op
   rebuild.** Native requests cut off through the build record; WASM requests
   were never eligible. A wall sample of a 3.2 s unchanged WASM build spent
   0.77 s restoring the final link and its result, 0.55 s admitting the runtime
   pair, 0.39 s admitting the backend, 0.23 s on the runtime import closure,
   0.21 s on backend cache setup and 0.12 s admitting the facts tool. No linker
   or optimizer ran. A native JSON build without its record pays the same class
   of cost (4.29 s).
4. **Two reads still cost records.** Every checkout of one artifact root
   shares the Rust product store, and any of them rewrites its retention ledger
   on renewal: another lane's renewal made one unchanged build per arm miss
   (4.7-8.1 s). A linked WASM build stages its deployment generation in a
   directory beside the manifest; the build that relinked could not record, so
   the first unchanged build after an edit paid a full rebuild. Both are fixed.
5. **Edits and cold builds are first-build work, not missing cutoff.** A
   one-statement WASM edit spends 12 s of an 18 s sampled build in the final
   link: `wasm-opt -Oz` over the whole 11 MB linked module and facts scans of
   the unchanged runtime (HF-26). A cold build re-lowers and recompiles the
   stdlib and links once (HF-26, HF-371). The output must change in both.
6. **A native JSON build relinks without a record.** Every native relink
   prints `ld: warning: ignoring duplicate libraries` (the Mach-O plan passes
   the runtime archive twice on purpose), and a JSON result with stderr refuses
   the record. The next unchanged build skips the link and records (4.3 s), so
   JSON reaches its 1.1 s cutoff one build later than plain. Not changed here.
7. **The shared source-record store dominated edits under contention.** The
   frontend's store (`MOLT_CACHE/toolchain_source_records/<schema>/records.sqlite`)
   holds every lane's records on this host. It had grown to 12.5 GB and grew
   13 MB a minute while lanes built. Under I/O contention on the external
   volume, one-statement edits took 94-216 s instead of 18-40 s: 793 of 798
   main-thread samples of one such build sat in SQLite `readDbPage` → `pread`.
   A fresh cache for this witness holds 110 MB. Retention of that store belongs
   to its owners (HF-513, HF-582); a single user's store stays small.

## Discriminating experiment

The explanation predicts that the same unchanged WASM request, once eligible
for the record, finishes near native-record cost with byte-identical outputs,
and that no linker or optimizer runs. Inherent WASM output work (publishing the
11 MB family, link validation) would instead keep a hit in the seconds. The
matched blocks confirm the prediction: 15 of 16 after-arm builds hit in
0.53-0.64 s against 0.32-0.40 s for native and 5.8-29.5 s before; the only miss
was finding 4's store renewal. The WASM hit costs more than the native one
because its record is larger (2.35 MB: 6,854 files, 520 directories and 6,441
probes, against about 1.4 MB).

## Fix

The record admits the default `wasm` target for plain requests, linked or not,
without split runtime, precompile or a named linked output. The result owner
reports the published family as a typed `BuildProducts` fact. Record schema
v10 keeps each product's token; a hit requires them all and names the module a
user runs. Inputs that reached no authority before now do: the wasm-opt PATH
search goes through the shared executable search;
`tools/wasm_allowed_imports.txt`, read only by wasm-ld, is a producer-bound link
input that also keys the final-link cache, which a hand edit of that generated
file used to leave stale. The Rust product store and the runtime WASM
generation store are named product roots: their generations are immutable and
selected by identities the recorded inputs derive, and their retention ledgers
change with any checkout's use. Publication custody follows the staging-name
authority transitively, so a stage of a stage and the contents of a staged
directory are products.

Must-miss controls on real builds, each followed by a rebuild and then a hit:
an appended comment in the allowlist (miss on the file), a `wasm-opt` link
added to an earlier PATH directory (miss on that directory), a one-statement
edit (miss on the source; the next build hits in 0.59 s), and one byte appended
to `manifest.json` (miss on the product; the rebuild republished the original
bytes).

The no-op check follows [observed_inputs.md](observed_inputs.md), which owns
its rules and prior art. rustc incremental and Zig's cache reuse work inside a
changed build, the edit case (HF-26). Bun's `--compile` keeps no cache and pays
tens of seconds every time, the shape Molt's WASM no-op had.

## Cost

- A hit stats every recorded input and the four products and parses the
  record: 0.53-0.93 s for this WASM request (2.35 MB record) on this host.
- A recording build writes the record. Sampled on a WASM edit build:
  0.34 s rehashing consumed sources, 0.18 s checking probes and 0.14 s in the
  custody scan, which now resolves each observation once and scans only output
  directories. Sampled on a native edit build: 0.2 s. Each request keeps one
  record under `MOLT_CACHE/build_records`, trimmed after a week unused.
- A linked build hashes the 977-byte allowlist once more. Every program
  relinks once after this change, because the final-link key gained an input.
- The store roots add no work: they remove recorded inputs.
- Emitted programs are byte-identical.

## Not done

JSON WASM requests still run the pipeline: they need a typed WASM result to
replay, as native JSON has. The edit and cold costs belong to HF-26 (link and
facts scans), HF-371 and the stdlib content seed. The native JSON link warning
(finding 6) needs either a measured removal of the second archive pass or a
typed classification of that warning. The source-record store's growth
(finding 7) needs a retention decision by its owners.
