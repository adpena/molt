# WASM hosts: one host ABI, a Rust runtime, thin hosts

Status: design, revision 3 (2026-11-08). It went through Goldfish rounds 1 (codebase details,
zoom-out) and 2 (design), a local workerd probe, and an online survey of WASI 0.2/0.3, jco and
workerd. Owner: integrator. Rows: HF-790 (the Tier 1 Workers cell), V1-19 (runtime capability
enforcement) and HF-699 (§6, runtime callables as keys).

This design extends four authorities and duplicates none:
- `runtime/host_capabilities.toml` and [CAPABILITIES.md](../CAPABILITIES.md): operations, and the
  `targets` axis built from the deploy profiles;
- `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`: the import tables;
- `molt-runtime-vfs`: mounts and file grants;
- spec 0294 §5.2: the Molt Edge host matrix.

## Problem (measured 2026-11-08)

The product Workers path (`molt deploy cloudflare --dry-run`, split runtime) runs on local workerd
(pinned Wrangler 4.151.3). A hello program answers in 113 ms with CPython's output. A probe of
host-facing operations shows what users hit.

The cloudflare profile refuses to compile the probe: its `pure` list strips `lock_`, which
`_thread` needs at import. So the probe was built with the browser profile:

| Probe on workerd | Molt | CPython |
|---|---|---|
| `io.StringIO`, `io.BytesIO` | works | works |
| `/tmp` write and read, with `fs.read` and `fs.write` granted | works (JS VFS) | works |
| `open("/bundle/x")`, `os.listdir("/")` | `FileNotFoundError` with `[Errno 2] [Errno 2] open '...': No such file or directory (os error 44)` | the CPython message is `[Errno 2] No such file or directory: '...'` |
| `import subprocess`, `import threading` | `OSError: tzname unavailable` at import | the imports work; `Popen` raises `OSError(ENOTSUP)` and `start()` raises `RuntimeError` |
| the same imports, after a scratch patch that answers `UTC` for `tzname` | the imports work; the calls raise `NotImplementedError` | as above |

Native Molt is wrong too. On macOS, compared with CPython 3.12.16 on the same host:
- with `TZ=UTC`, Molt gives `tzname == ('UTC', '   ')`, while CPython gives `('UTC', 'UTC')`;
- with `TZ=Asia/Kolkata`, Molt gives `daylight == 1` and `tzname == ('IST', '+0630')`, while
  CPython gives `0` and `('IST', 'IST')`.

Molt reads libc's `tzname` and `daylight` globals, which describe the zone's whole history. CPython
samples the current year.

Causes:

1. **Seven hosts implement one ABI**, about 24,000 lines in all. They are `wasm/run_wasm.js`
   (Node, the differential-test host), `wasm/browser_host.js` (runs programs in a browser),
   `wasm/browser_embed.js` (calls exports from JS), the Workers JS emitted inline by
   `src/molt/cli/wasm.py`, `tools/wasm_worker_template.js` (no product caller), the edgebox
   example's `db-bridge.ts`, and the Rust Wasmtime host `runtime/molt-wasm-host`
   (`--precompile`, direct runs). Each one implements the
   `env` imports and WASI by hand, and they drift apart:
   - the Workers host returns `-ENOSYS` for about 35 imports, among them `molt_time_tzname_host`,
     although workerd reports time zone `UTC` (measured);
   - the embed host returns an empty `tzname` as success;
   - Node and the Rust host each compute the standard/daylight profile in their own code.
2. **The import ABI has no single table.** `[[strip_import_rule]]` lists host imports without
   signatures. The WASI signatures sit in `[[link_allowed_import]]`. The `env` signatures are
   hand-written Rust externs (`lib.rs:1050-1063`, `os_ext.rs:660-663`). Hosts also implement
   imports that no row lists (`fd_fdstat_get`, `clock_res_get`, `proc_raise`,
   `molt_isolate_import`, `molt_log_host`).
3. **Two file systems, both partial.** The Workers and browser hosts serve WASI `path_open` from a
   JS VFS (`wasm/molt_vfs_browser.js`). The runtime VFS (`molt-runtime-vfs`) routes only `open()`
   and the stat family (`open.rs:584`, `io.rs:402-435`). `listdir`, `unlink`, `mkdir` and fd-level
   I/O go through `std::fs` straight to WASI. VFS files have no descriptor (`open.rs:750-757`). The
   runtime VFS mounts nothing without a bundle (`load.rs:484`). IoMode is set but never read. The
   JS `molt_vfs_*` imports have no runtime importer.
4. **The `pure` list makes imports fail.** It strips intrinsics, and a stripped intrinsic fails at
   import ("intrinsic unavailable"). Its entries come in three kinds:
   - in-module computation: `time_`, `lock_`, `rlock_`, `chan_`, `promise_`, compression, hashing,
     `ast_`;
   - host operations: `process_`, `socket`, the `asyncio_` socket polls;
   - size stripping.

   CPython imports `time`, `threading` and `subprocess` on WASI and fails only at the unavailable
   call. Cloudflare and Fastly use `pure` (`build_output_layout.py:70,92`).
5. **Unavailable operations fail four ways, none of them CPython's:** `NotImplementedError`,
   `RuntimeError("process spawn failed")`, `RuntimeError("operation ... is unavailable")`, and a
   compile-time panic.
6. **The execution target is a string each host sets at run time.** Each host sets
   `MOLT_EXECUTION_TARGET`; the runtime defaults it to `wasi`, and Node claims `wasi`.

## Survey conclusions (sources in the lane's research report)

- workerd supports no components and no WASI p2 or p3; jco calls Workers unsupported.
  Cloudflare's direction is Emscripten (Rust Workers, Pyodide). WASI 0.3 shipped on 2026-06-11, but
  `wasmtime-wasi` p3 is still marked experimental, and `wasi:clocks/timezone` is a phase 2
  proposal. Fastly, the largest production user of components, keeps a custom host ABI on top of
  WASI.
- Decision: keep wasm32-wasip1 core modules and Molt's own ABI, shaped like WIT (typed results,
  resources, streams), so that a later move to WASI 0.3 is a regeneration. Check again in 2027-Q2
  whether Workers supports components, `wasmtime-wasi` p3 is no longer experimental, and wasip3
  threads exist.
- wasi-vfs (ruby.wasm) packs files into the module and serves WASI file calls inside it; WASI-Virt
  does the same for components. Molt's runtime VFS is the same pattern in Rust.

## Design

### 0. Rust owns behavior; hosts bind primitives

The Rust runtime owns every behavior: file systems, the fd table, grants, errno mapping, exception
shaping, unavailability and time-zone policy. A host binds only primitives that the runtime cannot
reach by itself: a clock, entropy, stdio, a local-time query, a socket from `connect()`, a fetch, a
websocket, a child process on Node. Native targets need no host. Behavior placed in Rust works on
every target.

### 1. One import table; hosts generated from it

One manifest table owns every host import: its module, name, typed signature and operations. It is
the existing `[[link_allowed_import]]` table, extended to the `env` imports and given operation
references; `[[strip_import_rule]]` folds into it. A row with no operation is always available
(clock, entropy, stdio). `tools/gen_wasm_abi.py` generates from the table:

- the Rust externs (replacing the hand-written ones);
- the Wasmtime host glue, which reuses `wasmtime-wasi` p1 for WASI and binds the `env` primitives
  to the same Rust functions native code uses;
- one JS import table per host: each row maps either to a primitive binding or, where the row's
  operation is unavailable on that host's target, to a generated binding that returns `ENOTSUP`.

The generator fails when a host binds an import its target does not admit, or misses one it admits.
`gen_host_capabilities.py` keeps owning `targets`; both generators read the registry. `node`
becomes a deploy profile, which puts it in the `targets` vocabulary.

JS primitive bindings live in one module per family (`time`, `fs`, `process`, `socket`, `websocket`,
`db`, `wasi-core`) and are shared by every JS host. The browser run host and the embed host stay;
they share the families and differ only in their entry point. The template and its generator are
deleted.

The migration goes family by family across all hosts. Each landing leaves one implementation of
its family.

### 2. The time family (first)

The runtime primitive is `local_zone(secs) -> (utc_offset_east_seconds, zone_label)`, which is what
`localtime` returns in `tm_gmtoff` and `tm_zone`. Native Unix calls `localtime_r`. A JS host
cannot supply it: `Intl` names come from CLDR, not tzdata. Measured on Node, Asia/Kolkata reads as
`GMT+5:30` and America/St_Johns as `GMT-3:30`, where CPython reads `IST` and `NST`. So on WASM, the
host supplies only the IANA zone name (`Intl.DateTimeFormat().resolvedOptions().timeZone`), and
the runtime computes offsets and abbreviations from embedded IANA data, the data libc and CPython
read. The browser and node profiles embed it. The cloudflare and wasi targets are UTC by target
fact, so they need neither the data nor the import. The one import `molt_time_zone_name_host`
replaces the three imports `molt_time_timezone_host`, `molt_time_local_offset_host` and
`molt_time_tzname_host`; the Wasmtime host answers UTC, as CPython's WASI build does.

Landed first (native, 2026-11-08): the sampling algorithm and the Windows branch in Rust. Molt
native now matches CPython in UTC, Asia/Kolkata, America/Chicago, Australia/Sydney,
America/St_Johns and Pacific/Chatham.

Rust implements CPython's `init_timezone` (`Modules/timemodule.c`) once, for every target. Without
a `tzname` declaration, CPython samples `t = (now / YEAR) * YEAR` and `t + YEAR / 2`, where
`YEAR = (365 * 24 + 6) * 3600`. Then:
- if the January offset is less than the July offset (the southern hemisphere), the two swap;
- `daylight` is 1 when the two offsets differ;
- an offset beyond 48 hours raises "invalid GMT offset".

That branch is what CPython 3.12.16 runs on macOS and on Linux (python-build-standalone):
`HAVE_DECL_TZNAME` is 0 on both, and both produce the outputs above. Windows CPython declares
`tzname` (`PC/pyconfig.h`) and has no `altzone`, so `altzone = timezone - 3600`, with names from
`GetTimeZoneInformation`. Molt follows the same split. `localtime` takes `tm_zone`, `tm_gmtoff` and
`tm_isdst` from the same primitive on every target.

Oracle: CPython on the same host, with `TZ` set to UTC, America/Chicago, Australia/Sydney and
Asia/Kolkata, compared with Molt native and Molt WASM on Node. On workerd the zone is UTC.

### 3. Profiles stop stripping importable code

The cloudflare and fastly profiles move from `pure` to `auto`. Imports then work as they do in
CPython, and an unavailable host operation fails at its call. `pure` stays as an explicit size mode
until doc 60's reachability replaces it (doc 60:460-462). Measure Falcon's linked size and
instantiation time under `auto` against the 64 MiB limit before landing.

### 4. Unavailable operations raise what CPython's WASI build raises

One runtime helper is used for every unavailable operation:
- process creation raises `OSError(errno.ENOTSUP, "<platform> does not support processes.")`;
- thread start raises `RuntimeError("can't start new thread")`;
- an unavailable socket call raises `OSError(ENOTSUP)`.

The four current forms migrate to this helper. `sys.platform` stays `wasi`, as in CPython's WASI
build.

At compile time, reaching an unavailable operation produces one warning per operation family, with
the shortest chain from user code. The HF-790 attribution already computes those chains.
`--strict-platform` turns the warning into an error and gets a public-contract entry.

The execution target becomes a typed build fact that the runtime reads from its own module, not a
string that a host sets.

### 5. Files: the runtime VFS grows its routes, then replaces the JS VFS

Before the JS VFS can go, the runtime VFS needs routes for `listdir`/`scandir`, `unlink`, `rename`,
`mkdir` and `rmdir`, descriptors for fd-level I/O and `fileno()`, `/dev/null` in DevFs, and an
IoMode that is read. Each host gets a mount set:
- Workers and the browser: `/bundle` read-only, `/tmp` in memory with the profile's quota, `/dev`;
- Node and Wasmtime: real preopens for `/tmp` and the working directory (IoMode Real);
- `/bundle` everywhere.

`/bundle` comes from today's `bundle.tar` asset, injected at instantiation; it is not packed into
the module, which would slow compilation and count against the 1 s startup limit. The quota becomes
a build fact. Reads of `/bundle` use the existing `open_read_shared` `Arc` path, so a large input
is not copied twice.

The `strerror` leak (`os error 44`) is fixed in the runtime's `io::Error` to `OSError`
conversion. Then the JS VFS, the dead `molt_vfs_*` imports and the JS TmpFs are deleted.

### 6. Runtime callables are keys; the WASM table reserves no runtime region (HF-699)

Settled after two Goldfish rounds (design revision 3, 2026-11-08). Row: HF-699.

**Problem.** Every split-runtime program (the `browser` profile always builds split) trapped on
its first `await asyncio.sleep` with `molt_call_indirect1 missing table entry at 2`. Slot 2 is base
1 plus `poll_table_slot` 1, `async_sleep_poll`. WASM reserves a fixed prefix of 190 slots: 29 poll
slots, 80 reserved runtime callables and 80 trampolines. The shared runtime links with
`--table-base=191`, so in a split build nothing fills slots 1 to 190. Linked builds fill them from
the backend. Split builds rely on JS hosts that dispatch the reserved region by name, and nothing
dispatched the poll region. Native has no reserved region: a runtime callable is a synthetic key
(`RUNTIME_CALLABLE_KEY_BASE`, `RUNTIME_POLL_CALLABLE_KEY_BASE`), and the runtime resolves it
through generated tables.

**Decision (E): one mechanism on every target.** A runtime callable or runtime poll is a key on
every target, and the runtime resolves keys through the generated tables. On wasm32 the resolved
Rust function pointer is the runtime's own table index, which wasm-ld assigns and the runtime's own
element segment publishes, in linked and split builds alike. The table holds app functions and the
runtime's own address-taken functions; no slot is reserved for runtime callables, so no producer,
publisher, gap mode or host dispatch exists for them. Rejected: (A) the runtime publishes the fixed
prefix, which needs a post-link step in the runtime generation identity and keeps a WASM-only
mechanism; (D) resolve runtime addresses through relocations, which gives an imported function a
second app slot unless a GOT-style import is added. The tool conventions and the component model's
shared-everything linking (`DynamicLinking.md`, `SharedEverythingDynamicLinking.md`) give each
function one slot, owned by the module that defines it. Emscripten's loader-side `GOT.func`
filling is the host dispatch this design deletes.

Global invariants:
- **K1. Keys name reserved runtime callables and runtime polls** on every target. Compact builtins
  and app-resolver entries keep table slots; giving them keys is a follow-up in HF-699.
- **K2. The runtime resolves keys, and every table entry rejects one.** The runtime calls a
  resolved Rust pointer directly, never through the `molt_call_indirectN` host import. Every
  `molt_call_indirectN` traps on an index above `u32::MAX` before `i32.wrap_i64`, which would turn
  a stray key into a live slot. The trap is the only `unreachable` in that function, so the
  trapping frame (named in a linked module's name section) identifies it.
- **K3. No reserved table region.** The shared runtime links at the default table base; no
  validator models a fixed prefix.
- **K4. Conventions come from keys, resolved once.** The poll argument and fixed arity versus argv
  come from the generated key tables, never from table-slot ranges.
- **K5. Keys and slots are distinct types** in the backend, and a key is emitted with `i64.const`.

```rust
// runtime/molt-runtime/src/builtins/functions/function_abi.rs (K2, K4)
/// Resolve a stored poll identity: a named key through the generated table, otherwise the
/// app's table index. Requires: poll_fn is a key or an app table index (K1).
pub(crate) fn runtime_poll_call_target(poll_fn: u64) -> Option<PollTarget>;
/// The variant fixes the argument convention: a runtime poll takes the task's object word, a
/// compiled poll its raw payload address (its table index on wasm32, its address on native).
pub(crate) enum PollTarget { Runtime(extern "C" fn(u64) -> i64), AppTableIndex(u64) }

// runtime/molt-backend-wasm/src/wasm/module_abi/ (K1, K5)
pub(crate) enum CallableIdentity { TableSlot(u32), RuntimeKey(u64) }
/// Seeds each runtime poll's identity with RUNTIME_POLL_CALLABLE_KEY_BASE + poll_table_slot.
fn seed_function_table_slots(&self, identities: &mut BTreeMap<String, CallableIdentity>);
// crate::wasm_table: WasmCallableTableTarget::{TableSlot(WasmCallableTableSlot), RuntimeKey(u64)}
// type_layout.rs (K2): each exported molt_call_indirectN compares its index with u32::MAX
// (one i64.gt_u) and traps before i32.wrap_i64.
```

Stages (each lands one whole family; no build sees two identities for one callable):
- **E1, runtime polls (fixes the trap; landed in lane hf699-split-runtime-keys).** The backend seeds
  poll keys; only a task constructor (`CallAsync`, `AllocTask`) may store a key. `call_poll_fn`
  resolves keys on every target and calls the runtime poll itself; `wasm_poll_slot`, the poll
  normalization and `wasm_table_base_fallback` are deleted. The K2 trap is in every
  `molt_call_indirectN`. The target-neutral key base is one generated constant
  (`molt_ir::runtime_callable_abi_generated::RUNTIME_POLL_CALLABLE_KEY_BASE`). The 29 poll slots
  stay in the layout, unused, so the base, the host ABI and the prefix fixtures change once, in
  E2. First check: the shared runtime member imports its table, and its one active element
  segment starts at 191 and holds `molt_async_sleep_poll` and the other polls.
- **E2, reserved callables.** Compiled code and function objects carry reserved callables as keys;
  argv unpacking for the 78 `UnpackArgs` callables follows native's path; the 2 `CallFrame`
  callables are called directly. The backend's reserved trampolines, the JS host dispatch (four
  host files and `cli/wasm.py`), the prefix, `--table-base=191`,
  `WASM_RESERVED_RUNTIME_CALLABLE_BASE`, the gap modes, `include_fixed_prefix`, the table-base
  publication chain, the prefix fixtures and spec 0400's callable-table admission protocol go.
  The census of every deleted site is in the design record
  (`logs/agents/hf699-split-runtime-keys/design-rev3.md`).

Tests (CPython is the oracle):
- `tests/test_wasm_runtime_heavy_regressions.py::test_wasm_runtime_polls_match_cpython`: sleep,
  an async generator through `anext`, and a task, built linked and split.
- `test_wasm_call_indirect_rejects_runtime_keys` in the same file: a key passed to
  `molt_call_indirect1` traps, and the function at the key's low 32 bits never runs; a call with
  that slot reaches it. The probe drives the app object each build emits (linked and split), since
  neither final module exports these functions: in a linked module wasm-ld binds the runtime's
  imports to them (its name section names the trapping frame `molt_call_indirect1`), and a split
  host serves those imports itself until E2.
- The repaired DB parity fixture (grants `db.read,time`, rethrows the operation error) no longer
  traps on split, but deadlocks: `run()` is synchronous and the DB host answers through `fetch`
  promises that cannot settle inside synchronous WASM. It lands with an asynchronous browser run
  (HF-699).
- E2: a builtin called through a function object (both arities and argv) on split and linked; no
  element row of an app or runtime targets a sentinel for a runtime callable; the
  instructions-retired A/B on the async and call-heavy benchmarks, linked WASM.

## Skeleton of the end state

Structure is part of the specification (Skeleton Coding, arXiv 2610.38214). Behavioral tests cannot
see the structural requirements this design exists for: one host-import table, behavior in Rust,
no duplicate host code. Seven drifting hosts can pass every output test. So those requirements live
here, as global invariants enforced by generated code and generator checks, and the tests specify
behavior.

The skeleton has three tiers. Tier 1 is signature stubs with one-paragraph docstrings. Tier 2 adds
local contracts to the high-complexity callables. Tier 3 is the global invariants, cited from
docstrings. Implementations keep these types, signatures and owners.

### Global invariants

- **I1. One host-import table.** Every host import has exactly one `[[host_import]]` row. Rust
  externs, the Wasmtime glue and every JS import table are generated from it. No hand-written
  extern or binding names a host import.
- **I2. Rust owns behavior.** A host binding is a total function of one host primitive. It
  contains no Python semantics: no errno policy, no fallbacks, no profile computation.
- **I3. Availability is one fact.** An import is available on a target exactly when every
  operation in its row is admitted by `host_capabilities.toml` for that target. The generator
  refuses a host table that binds an unadmitted import or misses an admitted one.
- **I4. No silent defaults.** A failed primitive is an error the caller sees, never a default value.
- **I5. One time-zone answer.** For a zone and an instant, every target computes `LocalZone` from
  the same IANA data (libc on native, embedded data on WASM).

### Runtime (Rust)

```rust
// runtime/molt-runtime/src/object/ops_sys_time.rs

/// What `localtime` reports for one instant. (I5)
pub(crate) struct LocalZone {
    pub(crate) utc_offset_east: i64,
    pub(crate) label: String,
}

/// The `time` module's zone constants.
pub(crate) struct ZoneProfile {
    pub(crate) timezone: i64,
    pub(crate) altzone: i64,
    pub(crate) daylight: i64,
    pub(crate) tzname: (String, String),
}

/// `Localtime` surfaces as OSError, `InvalidOffset` as RuntimeError("invalid GMT offset"). (I4)
pub(crate) enum ZoneProfileError { Localtime(String), InvalidOffset }

/// CPython's sampling `init_timezone`.
/// Requires: `sample` is `local_zone` for the current target.
/// Ensures: the samples are taken at t0 = (now / YEAR) * YEAR and t0 + YEAR / 2, where
///   YEAR = (365 * 24 + 6) * 3600; when January's westward offset is smaller, the two swap;
///   daylight == (the offsets differ); each name keeps at most nine bytes.
/// Raises: InvalidOffset when an offset exceeds 48 hours; Localtime when a sample fails. (I4)
pub(crate) fn zone_profile_from_samples(
    now: i64,
    sample: impl FnMut(i64) -> Result<LocalZone, String>,
) -> Result<ZoneProfile, ZoneProfileError> {
    unimplemented!("specification stub")
}

/// The primitive. (I5)
/// Ensures: on Unix, equals `localtime_r`'s tm_gmtoff and tm_zone (three spaces when tm_zone is
///   null); on WASM, equals `zone_data::local_zone(host_zone(), secs)`.
pub(crate) fn local_zone(secs: i64) -> Result<LocalZone, String> {
    unimplemented!("specification stub")
}

/// CPython's `local_to_seconds` (`_datetimemodule.c`), for naive datetime timestamps.
/// Ensures: for a local time inside a DST gap or fold, the result is CPython's on the same
///   zone data; molt-runtime-serial reaches it through `RuntimeVtable::time_local_offset_west`.
pub(crate) fn local_to_utc(local_secs: i64) -> Result<i64, String> {
    unimplemented!("specification stub")
}

/// wasm32 only, for the browser and node profiles. cloudflare and wasi are UTC by target fact.
mod zone_data {
    /// Ensures: equals what libc's `localtime` computes from the same IANA release. (I5)
    pub(crate) fn local_zone(zone: &ZoneId, secs: i64) -> LocalZone {
    unimplemented!("specification stub")
}
}
```

### Host-import table (TOML) and the time family's only JS

```toml
# runtime/molt-backend-wasm/src/wasm_abi_manifest.toml (I1, I3)
[[host_import]]
module = "env"
name = "molt_time_zone_name_host"   # writes the IANA zone name, e.g. "Asia/Kolkata"
params = ["u32", "u32", "u32"]      # buffer, capacity, out length
results = ["i32"]                   # 0, or a negative errno (I4)
operations = []                     # always available on targets that bind it
family = "time"
```

```js
// wasm/host/time.js: shared by every JS host; generated tables import it. (I1, I2)
/** Writes Intl's resolved IANA zone name; returns 0 or -ENOMEM. */
export const timeZoneNameHost = (memory) => (bufPtr, bufCap, outLenPtr) => {
  throw new Error("specification stub");
};
```

### Tests

- Local, per component: `zone_profile_tests` (sampling, swap, truncation, the 48-hour check,
  failure propagation); `zone_data::local_zone` against libc `localtime_r` for every zone and
  instant in a fixed table; generator tests that refuse an I3 mismatch.
- Compatibility (differential, CPython as oracle): `tests/differential/stdlib/
  time_zone_constants_*.py` under `TZ` on native, llvm, jit and WASM on Node; the workerd cell.

## The Tier 1 cell (HF-790)

A runner builds through the product path, starts the pinned Wrangler in local mode with its state
in a scratch directory, sends a request, and compares the response with CPython's output for the
same program. The first case is hello; the probe cases join as §2 to §5 land. The runner runs in
the scheduled tier.

## Decision for the owner: the Falcon witness

Falcon-OCR has 269.9 M parameters, 1.08 GB as fp32. A Workers isolate has 128 MB. The best published
lossless float coding (ZipNN, DFloat11) reaches about 11 bits per weight (~371 MB). int4 with scales
is ~134 MB. The KV cache of a 1,536 px page is ~410 MB as fp16. The deploy README reserves any such
trade for the owner, with measurements of both choices.

| Option | Infrastructure added | Tier 1 rows | Claim | Measurements |
|---|---|---|---|---|
| A. Driver on workerd; real-weight inference as WASM on Wasmtime and Node | none | unchanged | "compiles and instantiates on Workers; real-weight OCR runs as WASM on Wasmtime and Node" | workerd startup and first call; OCR text and CPU on Wasmtime |
| B. A smaller OCR model in one isolate (PP-OCRv5 mobile ~21 MB; TrOCR-small int8 plus detection ~71 MB) | none | model changes | "runs OCR on Workers" for that model | accuracy against Falcon on the fixed image; isolate memory |
| C. Falcon int8 pipelined across about four isolates | service bindings; hidden-state RPC | unchanged | "runs Falcon on Workers" | int8 accuracy; per-token RPC cost (unmeasured); bounded KV |
| D. WASM Worker in front of native Molt in a Cloudflare Container | Durable Object, Container image, a second deploy path | inference moves to native | "served on Cloudflare" | Container start and request latency |

I lean to A for v0.1: it adds no infrastructure, keeps the model on WASM and keeps the claim
honest. B adds an in-isolate OCR demonstration if you want one.

## Stages

0. Owner: the Falcon decision. It does not block stages 1 to 4.
1. The time family (§2) across the runtime and all seven hosts; cloudflare and fastly move to
   `auto`
   (§3), with Falcon's size measured; the workerd runner with hello.
2. Delete the template; one import table with generated Rust externs (§1); unavailable errors and
   the typed execution target (§4).
3. The VFS routes, mount sets and `strerror` fix (§5); then delete the JS VFS.
4. The process, socket, websocket and db families; compile-time warnings and `--strict-platform`;
   the chosen Falcon cell; the cloudflare optimization level, `O3` against `Oz`, chosen by
   measurement.
