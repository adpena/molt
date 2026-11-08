# WASM hosts: one host ABI, a Rust runtime, thin hosts

Status: design, revision 3 (2026-11-08). It went through Goldfish rounds 1 (codebase details,
zoom-out) and 2 (design), a local workerd probe, and an online survey of WASI 0.2/0.3, jco and
workerd. Owner: integrator. Rows: HF-790 (the Tier 1 Workers cell) and V1-19 (runtime capability
enforcement).

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

1. **Six hosts implement one ABI**, about 24,000 lines in all. They are `wasm/run_wasm.js`
   (Node, the differential-test host), `wasm/browser_host.js` (runs programs in a browser),
   `wasm/browser_embed.js` (calls exports from JS), the Workers JS emitted inline by
   `src/molt/cli/wasm.py`, `tools/wasm_worker_template.js` (no product caller), and the Rust
   Wasmtime host `runtime/molt-wasm-host` (`--precompile`, direct runs). Each one implements the
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

## Skeleton of the end state

Structure is part of the specification (Skeleton Coding, arXiv 2610.38214). Implementations must
keep these types, signatures and owners; the differential tests (`tests/differential/stdlib/
time_zone_constants_*.py`, the workerd cell) specify the behavior.

```rust
// runtime/molt-runtime/src/object/ops_sys_time.rs: one implementation for every target.
pub(crate) struct LocalZone { pub(crate) utc_offset_east: i64, pub(crate) label: String }
pub(crate) struct ZoneProfile {
    pub(crate) timezone: i64, pub(crate) altzone: i64, pub(crate) daylight: i64,
    pub(crate) tzname: (String, String),
}
pub(crate) enum ZoneProfileError { Localtime(String), InvalidOffset } // OSError, RuntimeError
/// CPython's sampling `init_timezone` (Unix, WASM). Windows: CPython's declared branch.
pub(crate) fn zone_profile_from_samples(
    now: i64, sample: impl FnMut(i64) -> Result<LocalZone, String>,
) -> Result<ZoneProfile, ZoneProfileError>;
/// The primitive. Unix: `localtime_r`. WASM: `zone_data::local_zone(host zone, secs)`.
pub(crate) fn local_zone(secs: i64) -> Result<LocalZone, String>;
/// CPython's `local_to_seconds` (`_datetimemodule.c`), shared with molt-runtime-serial
/// through `RuntimeVtable::time_local_offset_west`.
pub(crate) fn local_to_utc(local_secs: i64) -> Result<i64, String>;

// wasm32 only: offsets and abbreviations from embedded IANA data, for targets whose
// hosts have a local zone (browser, node). cloudflare and wasi are UTC by target fact.
mod zone_data { pub(crate) fn local_zone(zone: &ZoneId, secs: i64) -> LocalZone; }
```

```toml
# wasm_abi_manifest.toml: the one host-import table (signatures, operations, family).
[[host_import]]
module = "env"
name = "molt_time_zone_name_host"   # IANA name, e.g. "Asia/Kolkata"
params = ["u32", "u32", "u32"]      # buffer, capacity, out length
results = ["i32"]                   # 0, or a negative errno
operations = []                     # always available
family = "time"
```

```js
// wasm/host/time.js: the time family's only JS, shared by every JS host.
export const timeZoneNameHost = (memory) => (bufPtr, bufCap, outLenPtr) => { /* Intl ... */ };
```

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
1. The time family (§2) across the runtime and all six hosts; cloudflare and fastly move to `auto`
   (§3), with Falcon's size measured; the workerd runner with hello.
2. Delete the template; one import table with generated Rust externs (§1); unavailable errors and
   the typed execution target (§4).
3. The VFS routes, mount sets and `strerror` fix (§5); then delete the JS VFS.
4. The process, socket, websocket and db families; compile-time warnings and `--strict-platform`;
   the chosen Falcon cell; the cloudflare optimization level, `O3` against `Oz`, chosen by
   measurement.
