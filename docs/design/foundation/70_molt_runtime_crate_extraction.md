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

2026-07-08 C1 follow-up: runtime-local bind/table constants, WASM table-base
state, and runtime counters now live in `molt-runtime-constants`.
`molt-runtime` privately re-exports the satellite and keeps only the
`molt-codegen-abi`-derived constants in the fan-in crate, so the new satellite
has no workspace dependencies and does not widen the build graph. The old
in-crate `constants.rs` module is deleted. Proof rows:
`20260708T030343-c1-runtime-constants-clippy-20260708c-eafc9391fdee4200`
passed the satellite lint boundary,
`20260708T031005-c1-runtime-constants-fanin-j1-20260708c-b965c77a0d7746fb`
passed the dependency-cleanup fan-in check with max blast radius still 30, and
`20260708T031427-c1-runtime-constants-fanin-postrebase-20260708a-e9b5ccca7d264560`
passed after rebasing over the importlib-platform and stdlib-surface commits.
The real native compiler proof
`20260708T030216-c1-runtime-constants-native-hello-20260708a-d5dd9f355fc9448c`
passed `python -m molt build examples/hello.py --target native --profile dev
--out-dir <clean-dir> --json`. The repeated nested memory-guard orphan-cleanup
warning is DX evidence for guard lifecycle cleanup, not a constants authority
failure.

2026-07-08 C1 follow-up: the pure `fnmatch` matcher, byte matcher,
normcase, and regex translation authority now lives in
`molt-stdlib-text::fnmatch`. `molt-runtime` keeps the object/ABI entrypoints in
`builtins/functions_fnmatch.rs` and `builtins/fnmatch.rs`, but both surfaces now
import the same stdlib-text implementation instead of carrying duplicate
runtime-local behavior. The old `stdlib_fs_extra`/`glob::Pattern` lane and its
fail-closed `false` fallback are deleted, and the stale
`functions_re::CharClassParse` alias disappeared with the old in-runtime
matcher. Queue row
`20260708T031128-c1-stdlib-text-fnmatch-satellite-20260708b-846028c322c6481c`
passed `cargo test -p molt-stdlib-text` in 16.5s. Runtime fan-in row
`20260708T032554-c1-stdlib-text-fnmatch-fanin-20260708e-01990725cb5640d5`
passed `cargo check -p molt-runtime` in an isolated proof target in 170.5s; the
fresh target was required because the shared `compiler-build-resource` target
session reused stale runtime-facing `molt-stdlib-text` metadata from another
worktree while a newer test artifact already contained `fnmatch.rs`. That is
DX evidence: the compiler mutex fixed unsafe parallel heavy builds, but the
target-session key still needs enough worktree/source-epoch isolation to avoid
cross-worktree stale fingerprints. After rebasing over the runtime-constants
origin/main commits, row
`20260708T033407-c1-stdlib-text-fnmatch-satellite-postrebase-20260708a-71d224bdac52460d`
passed the satellite tests in 18.7s, and row
`20260708T033516-c1-stdlib-text-fnmatch-fanin-postrebase-20260708a-b8b0b492c84e4cb0`
passed the runtime fan-in in 172.5s. The fan-in rows also repeated the
`nested-memory-guard-orphan-cleanup` warning. After rebasing again over the
async scheduler split on origin/main, row
`20260708T034048-c1-stdlib-text-fnmatch-fanin-final-20260708a-dc2486d556ec441d`
passed the final runtime fan-in in 114.9s. Receipts:
`D:\Molt\target\sessions\proof-rust-67233f77c6b3-cargo-mo\.molt_state\quarantine\cargo_incremental\20260708-032843-pid5776-orphaned_processes_cleaned\receipt.json`
and
`D:\Molt\target\sessions\proof-rust-2862251e65dc-cargo-mo\.molt_state\quarantine\cargo_incremental\20260708-033808-pid26036-orphaned_processes_cleaned\receipt.json`
and
`D:\Molt\target\sessions\proof-rust-d6f2053093bf-cargo-mo\.molt_state\quarantine\cargo_incremental\20260708-034241-pid4740-orphaned_processes_cleaned\receipt.json`.

2026-07-08 C1 follow-up: refcount verification diagnostics now live in
`molt-runtime-audit::refcount_verify`. `molt-runtime` keeps the public module
name only as a re-export, and its `refcount_verify` feature forwards to the
audit satellite. This removes the diagnostic tracking map and adversarial
underflow/leak tests from the runtime fan-in crate without touching object
storage or refcount implementation. Queue row
`20260708T032526-c1-refcount-verify-audit-satellite-20260708a-858ae231b1864674`
passed `cargo test -p molt-runtime-audit --features refcount_verify
refcount_verify`. The first parent fan-in row,
`20260708T032639-c1-refcount-verify-runtime-fanin-20260708a-32d9587df8fd4df2`,
was an invalid proof shape: `--no-default-features --features refcount_verify`
configured out `builtins`, so generated intrinsic resolvers failed before
reaching the moved boundary and then took 683s to close as stale. The corrected
row,
`20260708T034218-c1-refcount-verify-runtime-fanin-20260708b-aed2145d003a4417`,
passed `cargo check -p molt-runtime --no-default-features --features
stdlib_micro,refcount_verify -j1` in 59.531s. After rebasing across the later
fnmatch text-authority extraction, row
`20260708T035749-c1-refcount-verify-runtime-fanin-post-fnmatch-20260708a-15f4bb3b660140f0`
passed the same parent fan-in proof in 53.5s. Treat the stale invalid row and the
repeated nested memory-guard cleanup warning as proof-queue/Cargo-lock DX defects
to drive down separately, not as refcount authority failures.

2026-07-08 C1 follow-up: pure RFC 3492 `encodings.punycode` encode/decode
behavior now lives in `molt-stdlib-text::punycode`. `molt-runtime` keeps only
the ABI/object bridge in `builtins/punycode.rs`, importing the shared
`punycode_{encode,decode}_impl` authority. The module is always available from
`molt-stdlib-text`, like codec registry facts, because the runtime depends on
the text crate with `default-features = false` and the punycode intrinsics are
not a heavy `stdlib_text` surface.

Proof rows:
`20260708T040103-c1-stdlib-text-punycode-satellite-20260708b-680f230e36aa4b4a`
passed `cargo test -p molt-stdlib-text --lib` in 32.0s, and
`20260708T040238-c1-stdlib-text-punycode-fanin-20260708a-80dc7458ca614730`
passed `cargo check -p molt-runtime` in 137.7s with a dependency edge on the
satellite row. After rebasing over the refcount-audit satellite on
`origin/main`, row
`20260708T041015-c1-stdlib-text-punycode-fanin-postrebase-20260708a-c013c193ee6a422a`
passed the runtime fan-in in 142.7s. The first attempted single-test proof,
`20260708T040049-c1-stdlib-text-punycode-satellite-20260708a-157af34190f74bd5`,
was rejected by the queue with `queue-cold-single-cargo-proof`; that is useful
DX policy signal, not a code failure, and pushed this lane to prove the whole
crate lib shard instead of paying a cold compile for one test filter. The
runtime fan-in repeated the nested memory-guard orphan-cleanup warning with
receipt
`D:\Molt\target\sessions\proof-rust-772c70152a7d-cargo-mo\.molt_state\quarantine\cargo_incremental\20260708-040457-pid23588-orphaned_processes_cleaned\receipt.json`.

2026-07-08 C1 follow-up: UUID byte-generation support now lives in
`molt-runtime-platform::uuid_support`. The platform satellite owns UUID node
state, version/variant byte construction, MD5/SHA1 namespace hashing for UUID3
and UUID5, and UUID1 clock-sequence/timestamp state. `molt-runtime` keeps only
the Python object-facing entrypoints in `builtins/platform_env_ffi.rs`: argument
conversion, capability/audit checks, exception raising, and bytes-object
allocation. The move removes the UUID state and MD5 dependency from the parent
runtime fan-in while leaving importlib SHA1/SHA256 hashing explicit in the
runtime importlib files that still own that bootstrap behavior. Queue row
`20260708T040903-c1-platform-uuid-support-satellite-20260708a-2864faa6fd884035`
passed `cargo test -p molt-runtime-platform uuid_support -j1` in 25.9s. The
first parent fan-in row,
`20260708T040919-c1-platform-uuid-support-fanin-20260708a-8284b97d7465400d`,
correctly failed before reaching the intended assertion because
`platform_importlib_support.rs` still inherited `UNIX_EPOCH` from
`platform.rs`; the extraction made that implicit dependency visible. The
corrected row,
`20260708T041320-c1-platform-uuid-support-fanin-20260708b-60537db2c9564f52`,
passed `cargo check -p molt-runtime -j1` in 72.1s after the importlib support
file owned its time import explicitly. After rebasing across the WASM constant
materialization split, row
`20260708T042038-c1-platform-uuid-support-fanin-post-wasmconst-20260708a-57b38262dd0a4cc2`
passed the same runtime fan-in proof in 70.2s.

2026-07-08 C1 follow-up: the remaining pure `textwrap` residuals now live in
`molt-stdlib-text::textwrap`. `textwrap_shorten_impl` owns whitespace collapse
and placeholder truncation, while `textwrap_indent_{default,result}_impl` owns
line splitting and prefix assembly for both default and callable-predicate
indent. `molt-runtime` keeps only argument conversion, string allocation,
exception handling, and the Python callable predicate closure in
`builtins/functions_textwrap.rs`; the no-predicate path and the callable path
both share the stdlib-text line assembly authority.

Proof rows:
`20260708T043018-c1-stdlib-text-textwrap-residual-satellite-20260708a-9f43062a6aae4f94`
passed `cargo test -p molt-stdlib-text --lib` in 23.9s, and dependent fan-in row
`20260708T043031-c1-stdlib-text-textwrap-residual-fanin-20260708a-f2a010491ae8456b`
passed `cargo check -p molt-runtime` in 159.9s.

DX signal recorded during this cut: the required
`tools/structural_audit.py --check` preflight was run once before drift was
found, again after fast-forwarding to current `origin/main`, and again before
commit; all three returned `structural ratchet OK (684 findings; 0 metric(s)
improved)` but each took multiple minutes. Keep the C1 ratchet, but make this
board-required reflex incremental or cache-aware enough that decomposition
agents can run it at the required cadence without losing a full edit loop to
unchanged findings.

2026-07-08 C1 follow-up: host environment snapshot state, process-environment
state, locale state, target platform labels, locale encoding labels, and WASI
environment collection now live in `molt-runtime-platform::env_support`.
`molt-runtime` keeps the Python object-facing `platform_env_ffi.rs` ABI surface:
object conversion, exception raising, audit/capability checks, and bytes/string
allocation. The parent runtime reexports the platform support functions only for
the existing runtime bridge and stdlib consumers, so `molt-runtime-http` and
`shutil.which` continue to read the same shared environment authority without
owning that state in the parent god-crate.

Queue row
`20260708T042955-c1-platform-env-support-satellite-20260708b-4768c84fe60f4078`
passed `cargo test -p molt-runtime-platform env_support -j1` in 41.5s. Parent
fan-in row
`20260708T043046-c1-platform-env-support-fanin-20260708b-238110cf206e4c05`
passed `cargo check -p molt-runtime --features stdlib_micro -j1` in 627.9s with
a dependency edge on the satellite row. Final exact-tree row
`20260708T044428-c1-platform-env-support-fanin-final-20260708a-d170a8ea852b4877`
passed the same parent fan-in proof in 58.9s after the stale
`fill_os_random` import was removed and the warmed target dir was reused. After
rebasing over the textwrap residual extraction on `origin/main`, row
`20260708T044824-c1-platform-env-support-fanin-post-textwrap-20260708a-012cbb0b976940c5`
passed the current-runtime fan-in proof in 65.5s. The first two rows,
`20260708T042833-c1-platform-env-support-satellite-20260708a-7c5f50ee231e42cf`
and
`20260708T042834-c1-platform-env-support-fanin-20260708a-8b8ff1527cb9440c`,
were invalid proof shapes: the queue-owned Cargo lane passes arguments after
`cargo`, so the command must be `test --package ...` or `check --package ...`,
not `--package ... test/check`. Treat those rows as queue-DX evidence and not as
runtime failures. The 41.5s satellite proof versus 627.9s first parent fan-in
is a measured C1/R5 build-throughput signal: platform authority extraction is
cheap, but every remaining parent-runtime consumer still pays the god-crate tax
until the runtime fan-in is decomposed further. The warmed 58.9s final row shows
the queue target reuse helps, but it does not erase the structural parent-crate
tax.

2026-07-08 C1 follow-up: pure `stat` mode support now lives in
`molt-runtime-platform::stat_support`. The platform satellite owns POSIX/Windows
mode constants, target-gated 3.13 stat constants, `S_IFMT`/`S_IMODE`, `S_IS*`
predicates, and `filemode` text formatting. `molt-runtime` keeps only
target-version lookup, Python object conversion, exported ABI entrypoints, and
string/tuple allocation in `builtins/functions_stat.rs`.

Proof rows: the first satellite row
`20260708T044756-c1-platform-stat-support-satellite-20260708a-acc347d6b41e4da5`
failed at compile time because the new test used an undefined shorthand
`S_IRWXU`; `tools/proof_queue.py diagnose --append-note` recorded that as an
E0425 test defect, and dependent fan-in row
`20260708T044809-c1-platform-stat-support-fanin-20260708a-375a85464efa46c3`
correctly stayed blocked. Corrected satellite row
`20260708T045000-c1-platform-stat-support-satellite-20260708b-334c2e4aba514327`
passed `cargo test -p molt-runtime-platform -j1` in 30.6s, and dependent fan-in
row
`20260708T045012-c1-platform-stat-support-fanin-20260708b-d922a660a2da4746`
passed `cargo check -p molt-runtime` in 149.5s. After rebasing over the WASM
state-remap split on `origin/main`, post-rebase satellite row
`20260708T045944-c1-platform-stat-support-satellite-postrebase-20260708a-cabe43cba49148e7`
passed `cargo test -p molt-runtime-platform -j1` in 42.8s, and post-rebase
fan-in row
`20260708T045954-c1-platform-stat-support-fanin-postrebase-20260708a-3426931702ba4d16`
passed `cargo check -p molt-runtime` in 157.4s.

2026-07-08 C1 follow-up: path-list de-duplication and path-list splitting for
platform/importlib bootstrap now live in
`molt-runtime-platform::importlib_support`. The platform satellite owns
`append_unique_path`, `append_unique_path_hashed`, and `split_nonempty_paths`,
including direct tests for empty-entry filtering and stable de-duplication.
`molt-runtime` keeps only a reexport so existing platform, importlib, and
importlib.resources consumers continue to share one runtime-independent helper
authority while the next path-primitive cut is staged.

Queue row
`20260708T045706-c1-platform-importlib-path-list-satellite-20260708a-bc08ffd280f24c39`
passed `cargo test -p molt-runtime-platform importlib_support -j1` in 30.3s,
and dependent parent fan-in row
`20260708T045752-c1-platform-importlib-path-list-fanin-20260708a-4474dccf606d49a2`
passed `cargo check -p molt-runtime --features stdlib_micro -j1` in 641.2s.
The proof confirms the reexport contract, and the timing again shows that tiny
runtime-independent helper moves are cheap in the satellite but still pay the
parent god-crate compile tax until more consumers leave `molt-runtime`.

2026-07-08 C1 follow-up: resource-tracked temporary byte arena allocation now
lives in `molt-runtime-resource::TempArena`. The resource satellite owns
chunk allocation, tracker charging/release, reset/drain semantics, and the
resource-limit regression tests. `molt-runtime/src/arena.rs` keeps only the
object-writing `ScopeArena` authority and a runtime-local tracker-release
helper for its aligned object chunks; JSON parsing and parser TLS import the
temporary byte arena through `crate::resource`.

Queue row
`20260708T052219-c1-temp-arena-resource-lib-post-importlib-20260708a-a72f71227e30408d`
passed `cargo test -p molt-runtime-resource --lib` in 13.2s. The first parent
fan-in row,
`20260708T041406-c1-temp-arena-runtime-fanin-20260708a-8ba16e6ba791412f`,
correctly failed because `ScopeArena` still referenced the removed helper; the
repair kept object-arena release local to the parent runtime instead of pulling
`ScopeArena` into the resource satellite. After rebasing over the textwrap,
platform environment, WASM state-remap, stat-support, and importlib path-list
extractions on `origin/main`, current-tree fan-in row
`20260708T052259-c1-temp-arena-runtime-fanin-post-importlib-20260708a-f1988eca92014bcc`
passed `cargo check -p molt-runtime` in 69.8s. The board-required real compiler
E2E row
`20260708T052557-c1-temp-arena-molt-build-e2e-post-importlib-20260708a-e92f4930ab084d66`
passed `uv run --active --project . --python 3.12 python -m molt.cli build
--profile dev --output tmp\c1_temp_arena_hello_post_importlib.exe
tests\harness\corpus\basic\hello.py` in 234.5s, producing the native hello
binary.

2026-07-08 C1 follow-up: pure text path helpers now live in
`molt-runtime-platform::path_text`. The platform satellite owns join, split,
basename/dirname, suffix/stem, normalization, relative-path, root splitting,
raw-byte join, and variable-expansion text rules. `molt-runtime` keeps the
PyObject conversion, capability checks, filesystem calls, glob iterator state,
and error translation in `builtins/io_path_utils.rs`, plus a reexport of the
single path-text authority for existing path/importlib consumers.

Proof rows: first satellite row
`20260708T052332-c1-platform-path-text-satellite-20260708a-18f967f05f4949d1`
compiled and failed one new test because the test asserted POSIX separators
while running the Windows path branch; `tools/proof_queue.py diagnose
--append-note` recorded that as a Rust test failure. Corrected satellite row
`20260708T052545-c1-platform-path-text-satellite-20260708b-0ec722a3f6bf4889`
passed `cargo test -p molt-runtime-platform path_text -j1` in 20.8s. Dependent
parent fan-in row
`20260708T052618-c1-platform-path-text-fanin-20260708a-c66ece8e0b7e4af0`
passed `cargo check -p molt-runtime --features stdlib_micro -j1` in 575.1s.
The board-required real-build E2E row
`20260708T053720-c1-platform-path-text-molt-build-e2e-20260708a-e7f127a007694df0`
passed `python -m molt.cli build tests/differential/basic/module_metadata.py
--target native --out-dir tmp/c1_path_text_e2e` in 830.7s. The timing is the
C1 tax in plain numbers: the new authority proves in seconds, while parent
integration and a tiny real build still spend minutes in the god-crate path.
After rebasing over the TempArena resource split and WASM state-dispatch splits
on `origin/main`, post-rebase fan-in row
`20260708T055613-c1-platform-path-text-fanin-postrebase-20260708a-4a74e5f3bd7b48a4`
passed `cargo check -p molt-runtime --features stdlib_micro -j1` in 74.7s.

2026-07-08 C1 follow-up: errno generation and socket constant payload authority
now live in `molt-runtime-platform::socket_constants`. The platform satellite
owns the non-WASM build-script generation of `errno` constants, the WASM errno
table, address-family constants, socket type/option/nameinfo/getaddrinfo
payloads, and the target-gated `SOCK_NONBLOCK`/`SOCK_CLOEXEC` flags.
`molt-runtime` keeps only `molt_errno_constants` and `molt_socket_constants`:
Python object allocation, tuple/dict caching, and the exported ABI entrypoints.
The parent runtime build script no longer emits `errno_constants.rs`.

Proof rows: initial satellite row
`20260708T052015-c1-platform-socket-constants-satellite-20260708a-47676256d1b34c52`
passed `cargo test -p molt-runtime-platform -j1` in 39.0s, with the existing
nested memory-guard orphan-cleanup warning and receipt
`D:\Molt\target\sessions\proof-rust-c97fcaf95bde-c1-platf\.molt_state\quarantine\cargo_incremental\20260708-052053-pid16864-orphaned_processes_cleaned\receipt.json`.
Initial parent fan-in row
`20260708T052025-c1-platform-socket-constants-fanin-20260708a-59b5c97c32774015`
passed `cargo check -p molt-runtime -j1` in 631.6s after a cold
compile-dominated path. After rebasing over the resource TempArena and WASM
dispatch splits on `origin/main`, satellite row
`20260708T053507-c1-platform-socket-constants-satellite-postrebase-20260708a-46b50befb79a498d`
passed in 41.9s. The first post-rebase fan-in row
`20260708T053519-c1-platform-socket-constants-fanin-postrebase-20260708a-dbccd56b726041d1`
finished the underlying `cargo check` with `guarded_exec` returncode 0, but the
proof queue terminalized it as `stale` rc=2 with no diagnostic signal; treat
that as a queue terminalization/diagnosis defect, not a code failure. Warm rerun
`20260708T054928-c1-platform-socket-constants-fanin-postrebase-20260708b-627ab09c53d4413b`
reused the same contention key and passed in 89.2s, again with the nested
memory-guard orphan-cleanup warning and receipt
`D:\Molt\target\sessions\proof-rust-ab483886f6d0-c1-platf\.molt_state\quarantine\cargo_incremental\20260708-055057-pid16220-orphaned_processes_cleaned\receipt.json`.

DX signal recorded during this cut: changing proof contention keys for adjacent
post-rebase reruns can accidentally force another cold parent-runtime target,
turning a small authority move into a 10-11 minute fan-in. Reusing the warmed
session/key brought the same parent check down to 89.2s. Also, the stale row's
"no diagnostic signals" result is a deterministic diagnosis gap: the log already
contained `guarded_exec: ... returncode=0`, but the queue surfaced only stale
terminalization.

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
