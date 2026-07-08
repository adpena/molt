# 70. `molt-runtime` Crate Extraction Contract

`molt-runtime` is the dominant runtime fan-in crate. It should keep wiring,
link-owned ABI exports, and lifecycle ownership, while coherent runtime
subsystems move into lower-layer crates that compile independently.

## Current Contract

- `runtime/crate_graph.toml` is the graph authority. Runtime satellites sit at
  layer 3 and may depend on `molt-runtime-core` / `molt-obj-model`; the layer-4
  `molt-runtime` crate fans them in.
- A split is valid only when it moves one real subsystem authority, deletes the
  old in-crate module tree, wires the feature/dependency owner, and adds the
  crate to the graph ratchet.
- Public widening must be exact. No blanket `pub(crate)` to `pub`; no back-edge
  from a satellite to `molt-runtime`.
- The proof for each split is a focused satellite check plus a real
  `molt-runtime` consumer check. Non-build gates do not prove the compiler can
  still link the runtime.

## Reserved Surfaces

Do not use this document as permission to touch frozen or in-surgery lanes.
`docs/agent/ORCHESTRATION.md` controls live ownership.

- `runtime/molt-cpython-abi/**` and static-extension ABI layout are
  orchestrator/subagent-owned.
- `runtime/molt-runtime/src/cpython_abi_hooks.rs` remains in the runtime fan-in
  until the import/module-state and C-extension lanes explicitly reopen it.
- `runtime/molt-runtime/src/object/**`, the native-call lane, and
  module-state/import surfaces are not extraction targets while the board marks
  them in surgery.

## Landed Satellite Pattern

Existing satellites include text (`molt-stdlib-text`), regex, math, graphlib, difflib, logging,
path, collections, itertools, compression, crypto, net, asyncio, serial, HTTP,
stringprep, XML, ipaddress, zoneinfo, protobuf, and Tk. The canonical pattern is:

1. Create `runtime/molt-stdlib-X` for stdlib modules, or `runtime/molt-runtime-X`
   for runtime services such as VFS, with workspace edition/rust-version.
2. Move the subsystem source into that crate; do not leave a duplicate module
   body in `molt-runtime`.
3. Move feature-owned third-party dependencies to the new crate when the
   subsystem owns them.
4. Depend on the satellite from `molt-runtime` and re-export the satellite only
   as the runtime-facing authority.
5. Add the crate to `runtime/Cargo.toml`, root `Cargo.toml` when appropriate,
   and `runtime/crate_graph.toml`.
6. Prove the satellite and the fan-in consumer.

## 2026-07-07 C1-A: VFS Extraction

`runtime/molt-runtime-vfs` owns the mount-oriented virtual filesystem:

- `bundle`, `caps`, `dev`, `file`, `snapshot`, `tmp`, and the VFS root API.
- `vfs_bundle_tar` now belongs to `molt-runtime-vfs`; `molt-runtime` forwards
  the feature instead of owning `tar`.
- `molt-runtime` re-exports the crate as `vfs`, preserving one VFS authority for
  state, IO, runpy, and sys consumers.

The only intentional visibility widening is `load_vfs()`: runtime state still
decides when VFS is lazily loaded, while the VFS crate owns how the state is
constructed.

Follow-up on the same C1 authority moved VFS load and host-injected bundle
custody out of the root module into `runtime/molt-runtime-vfs/src/load.rs`.
That file now owns quota parsing, native directory/tar loading, WASM injected
entries, and shared `/tmp` + `/dev` runtime mount construction; `lib.rs`
keeps only the root VFS API and public re-exports.

Rebased proof on `origin/main` `fbb1eae15`:

- `20260707T200553-c1-vfs-load-split-allfeatures-test-20260707e-74e66eaaa60f4b49`
  passed `cargo test -p molt-runtime-vfs --all-features`.
- `20260707T200643-c1-vfs-load-split-molt-runtime-check-20260707e-85414a30ee594bee`
  passed `cargo check -p molt-runtime`.
- `20260707T201403-c1-vfs-load-split-native-e2e-20260707a-afb8fe60d17d493a`
  passed a real native `molt build examples/hello.py --target native --profile
  dev --out-dir <clean-dir> --json`.
- `uv run --active --project . --python 3.12 pytest -q tests/test_browser_vfs.py`
  passed locally (`3 passed`).

The next native bundled-VFS proof exposed a reusable runtime authority gap, not
a VFS-specific defect. Row
`20260707T202635-c1-vfs-load-split-native-vfs-bundle-20260707a-8120253ff1884ba9`
failed because `open()` received a float default as its mode: frontend
`builtin_func` carried only the executable runtime symbol
(`molt_open_builtin`), while `molt_func_new_builtin_named` needs the Python
builtin name (`open`) to resolve default metadata. The fix is shared authority:
frontend builtin/runtime function emission now serializes a const-string name
operand plus `builtin_name`; IR/TIR preserve the metadata; Cranelift, LLVM, and
WASM use `molt_func_new_builtin_named` when the name operand is present; the raw
constructor remains only as the legacy no-name fallback. This is deliberately not
an `open` shim and not VFS-owned behavior. Proof:
`20260707T210140-c1-builtin-named-constructor-check-20260707a-2289cdd1acb544e6`
passed `cargo check -p molt-ir -p molt-backend-native -p molt-backend-wasm`.
Adversarial review tightened the same boundary: `builtin_name` and the
executable name operand are lockstep. `builtin_name` is rejected unless the
`builtin_func` op carries exactly one executable name operand, and a name operand
is rejected unless `builtin_name` metadata is present. LLVM now matches
Cranelift/WASM by taking the raw constructor on no-name ops, and WASM emits
`molt_func_new_builtin_named(name_bits, fn_ptr, trampoline_ptr, arity)` in the
runtime ABI order.

The rerun then moved the native bundled-VFS proof to a deeper shared IO blocker:
`20260707T210532-c1-vfs-load-split-native-vfs-bundle-20260707b-0837ae1419b048a9`
built successfully, then the compiled binary failed at startup with
`RuntimeError: memory backend missing`. Diagnosis: `TextIOWrapper` over the VFS
memory-backed binary buffer had `mem_bits == 0`; normal text files hid that
because `std::fs::File` ignores `mem_bits`, but VFS memory files require the
bytearray backing store. The fix routes shared read/write buffer operations
through the child buffer's `mem_bits` when the text wrapper itself has none.
This is reusable IO layering, not a VFS path special case. Compile proof:
`20260707T211459-c1-vfs-text-memory-backend-check-20260707a-d7b91ea23b2e475b`
passed `molt-runtime` + `molt-ir`; the remaining signal was the pre-existing
nested memory-guard orphan-cleanup DX warning.

Native bundled-VFS behavior then passed in
`20260707T212019-c1-vfs-load-split-native-vfs-bundle-20260707c-55d529fb7e0e4c4a`:
the compiled binary read `/bundle/data.txt`, read nested
`/bundle/nested/more.txt`, wrote and read `/tmp/out.txt`, and rejected a
write back into `/bundle` with `PermissionError`.

Security model: VFS bundle loading is sandbox-by-default. Browser/worker bundle
injection remains a byte-entry protocol that rejects absolute paths, `..`, NULs,
and quota overflow before mounting `/bundle`. Native directory bundles now treat
the host filesystem as untrusted input: the root and entries reject symlinks,
junctions, mount points, and other Windows reparse points by default, stop
ignoring `read_dir` errors, and track canonical visited directories to avoid
cycles. Full host-link access is still available for explicit unsafe native
development with exactly `MOLT_VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS=1`; unset or
`0` keeps sandbox behavior, and any other set value fails closed instead of
guessing truthiness. Unsafe access remains full-control opt-in, not ambient
behavior.

The split-runtime/browser VFS assertion row is blocked before it reaches VFS:
`20260707T200848-c1-vfs-load-split-wasm-split-vfs-adapter-20260707c-ee804b91561c476d`
fails while compiling `molt-runtime` for `wasm32-wasip1` because
`runtime/molt-runtime/src/cpython_abi_hooks.rs` references
`crate::c_api::PyDict_Items` and `PySet_{New,Size,Contains,Add,Discard}`, while
`runtime/molt-runtime/src/c_api/mod.rs` only re-exports the `cpython_compat`
helpers on non-wasm32. That blocker is CPython-ABI hook/runtime-wasm visibility
work, not VFS load authority.

2026-07-08 update: the wasm runtime visibility blocker is resolved by compiling
the existing Rust-callable `c_api::cpython_compat` helpers on wasm while leaving
the wasm C-linker export surface owned by `cpython_abi_wasm_exports` and the
split `molt-cpython-abi` archive. Queue row
`20260708T011334-c1-runtime-capi-wasm-helpers-20260708a-85a9f829b1fb4740`
passed `cargo check -p molt-runtime --target wasm32-wasip1
--no-default-features`. The first VFS/browser rerun,
`20260708T011731-c1-vfs-split-wasm-adapter-rerun-20260708a-670bf6e3a9e244d3`,
then moved the blocker to DX wall clock: the build fixture timed out at its
inner 900s guard after the split-runtime harness forced `CARGO_BUILD_JOBS=1`
and `MOLT_WASM_DISABLE_SCCACHE=1`. Removing those harness defaults and
delegating cargo parallelism/cache policy back to the DX/cargo authorities made
`20260708T014407-c1-vfs-split-wasm-adapter-rerun-20260708b-c67411edb0ca4105`
pass the selected split-runtime VFS/browser assertions in 717.62s
(731.094s queue elapsed). After rebasing over the runtime-platform extraction,
`20260708T020156-c1-vfs-split-wasm-adapter-rerun-20260708c-c37457ef2cc84382`
passed the same selected assertions in 582.10s (586.937s queue elapsed). The
next rerun on the frontend-only rebased base,
`20260708T021519-c1-vfs-split-wasm-adapter-rerun-20260708d-ec6826e94527497d`
passed in 96.23s (100.969s queue elapsed), producing `app.wasm`,
`molt_runtime.wasm`, `molt_vfs_browser.js`, `worker.js`, `manifest.json`, and
`wrangler.jsonc` under
`D:\Molt\tmp\pytest-temproot-24076-65bb3a0a97bb40919db51056fb8fea09\pytest-of-adpena\pytest-0\split_a0\out`.

Validation ladder for this split and the next C1 cuts:

- Structural proof: satellite crate test, then `cargo check -p molt-runtime`.
- Real compiler proof: queue-owned `python -m molt build examples/hello.py
  --target native --profile dev --out-dir <clean-dir> --json`; a cargo-only
  proof is not enough for a runtime extraction.
- Runtime behavior proof: queue-owned native bundle driver that reads nested
  `/bundle` files, writes `/tmp`, and rejects writes back into `/bundle`.
- Browser/split proof: `tests/test_wasm_split_runtime.py` and VFS browser
  assertions; current proof row
  `20260708T021519-c1-vfs-split-wasm-adapter-rerun-20260708d-ec6826e94527497d`
  is the citable C1 VFS split-runtime acceptance for this cut.
- Benchmark triage: `uv run --active --project . --python 3.12 python
  tools/bench_individual.py --bench bench_etl_orders.py --bench
  bench_json_roundtrip.py --samples 3 --warmup 1 --json-out
  bench/results/c1_vfs_load_<stamp>.json`.
- Citable performance gate, only for a release/perf claim: `uv run --active
  --project . --python 3.12 python tools/perf_scoreboard.py --set core
  --backend native --backend llvm --profile release-fast --samples 5 --warmup 2
  --repeat 5 --classify --require-quiescent`.
- Stress/endurance: run the loop/object stress differential files through queue
  custody, serializing compiler-build resources rather than creating a parallel
  rebuild storm.

DX signal recorded during this arc: the split proof repeated the
`nested-memory-guard-orphan-cleanup` warning on otherwise green cargo rows, an
early VFS test row hit an sccache server disconnect (`os error 10054`) before it
reached the changed crate, and even local Python `--help` probes were slow
enough to notice while the native E2E build was active. Fresh current-main
landing prep added a harder number: the required `tools/tree_drift_check.py
--witness --fetch` reflex, invoked through `uv run --active`, spent 4m58s
installing 57 packages before returning the one-line verdict. Treat those as
iteration-loop evidence for the C1/R5 optimization queue, not as VFS correctness
failures. Drift checks should become dependency-light or run from a stable warm
RunContext environment so the board-required reflex stays cheap enough to use.
The split-runtime harness itself also contained a silent degrade-to-slow path:
it forced serial cargo and disabled wasm sccache even when the board and DX
authority expected warm/shared builds. Any future WASM test helper should opt
out of cache/parallelism only through explicit operator env, never as a default.

2026-07-08 C1 follow-up: the runtime-independent `importlib` archive/path and
metadata parsers now live in `molt-runtime-platform::importlib_support`.
`molt-runtime` keeps the object/ABI/bootstrap bridge in
`builtins/platform_importlib_support.rs`, but imports the pure helpers from the
platform satellite instead of carrying local duplicate authority. Queue row
`20260708T023119-c1-platform-importlib-support-satellite-20260708a-0e194159ce86428a`
passed `cargo test -p molt-runtime-platform`, proving the satellite-owned
parser behavior directly. The first fan-in row,
`20260708T023132-c1-platform-importlib-support-fanin-20260708a-e7e5f9fad75945b6`,
was an invalid proof shape: `cargo check -p molt-runtime --no-default-features`
configured out `builtins` and failed before reaching this importlib cut. The
next default-feature rerun,
`20260708T024928-c1-platform-importlib-support-fanin-20260708b-b6b1663927d746ae`,
failed because the new Rust module was still untracked and therefore absent
from the queue proof snapshot. After staging the exact C1 pathspec, rerun
`20260708T025204-c1-platform-importlib-support-fanin-20260708c-3f5c5eb82f664720`
passed `cargo check -p molt-runtime` in 89.609s. The final warning-cleanup
rerun,
`20260708T025448-c1-platform-importlib-support-fanin-20260708d-e198f95e263342b9`,
passed the same default-feature runtime fan-in in 89.938s.

## Next Decomposition Order

1. Continue legal subsystem extractions that avoid reserved lanes, starting with
   runtime support surfaces that have narrow call sites and no object/module
   back-edge.
2. Extract async runtime only after its task/object/state dependencies are
   mapped and the split can avoid a `molt-runtime` back-edge.
3. When the board opens `object/**`, sever object-to-builtins back-edges and
   extract the object/core authority before carving `builtins`.
4. Split `builtins` by CPython module family only after core/object ownership is
   stable and the feature graph can tree-shake each family.
