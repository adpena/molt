<!-- Gap-surface recon (background agent, 2026-06-04). Read-only audit; codebase = sole source of truth. -->
<!-- User directives driving this: "cpython >= 3.12 surface API coverage parity issue" + "frontier stdlib and gpu and other work is far from finished". -->

# CPython ≥3.12 Surface-API / Frontier-Stdlib / GPU Gap Audit

## LANE 1: CPython >= 3.12 BUILTIN/STDLIB SURFACE-API COVERAGE PARITY

### 1.1 Stdlib Module Coverage Inventory

**Location:** `/Users/adpena/Projects/molt/src/molt/stdlib/`

**Total stdlib modules present:** 280+ Python files (full CPython 3.12+ stdlib mirror structure)

**Module presence/coverage matrix:**

| Category | Module Status | Coverage | Notes |
|----------|---------------|----------|-------|
| **Core Builtins** | builtins.py | Partial | 22.7KB; intrinsic-backed bootstrap for classmethod/staticmethod/property; missing: complex literals, some formatting builtins |
| **Core Types** | int/str/list/dict/set/tuple/bytes | Intrinsic-backed | Via _intrinsics.py (5.8KB); all basic methods present |
| **Math/Numeric** | math.py | Partial | 5.7KB; sumprod (3.12 new) implemented; some transcendentals pending float-determinism policy |
| | random.py | Partial | 11.3KB; core distributions OK; extended test vectors pending |
| | statistics.py | Partial | 14.8KB; basic functions OK; NormalDist pending |
| | cmath.py | Partial | 5.0KB; complex math pending complex literal support |
| | decimal.py | Full | 39.8KB; complete 3.12 API |
| | fractions.py | Full | 9.2KB |
| | operator.py | Partial | 1.9KB; fast-path intrinsics pending |
| | numbers.py | Partial | 5.8KB; all abstract base methods raise NotImplementedError (by design) |
| **Collections** | collections/ | Partial | deque/Counter/defaultdict pending intrinsic optimizations |
| | array.py | Partial | 6.8KB; deterministic layout via intrinsics pending |
| | heapq.py | Partial | 2.3KB; randomized stress testing pending |
| | bisect.py | Partial | 0.8KB |
| | itertools.py | Partial | 5.5KB; core opcodes OK; fusion semantics pending |
| **String/Text** | string.py | Full | 6.7KB |
| | stringprep.py | Full | 3.3KB |
| | re.py | Partial | 4.6KB wrapper; native engine has: literals, `.`, classes/ranges, groups/alternation, greedy/non-greedy, IGNORECASE/MULTILINE/DOTALL; advanced (lookahead, named groups, flag scoping) raise NotImplementedError |
| | difflib.py | Full | 28.1KB |
| | textwrap.py | Full | 14.9KB |
| | unicodedata.py | Full | 7.6KB |
| **IO/Filesystem** | io.py | Partial | 4.1KB; pure-Python wrapper; _pyio.py 91.7KB has most methods, some stream operations stubbed |
| | os.py | Partial | 43.8KB; core ops (listdir, walk, stat, rename, mkdir) via intrinsics; dir_fd unsupported (12x NotImplementedError); utime(ns=...) unsupported |
| | pathlib.py | Full | via molt-runtime-path Rust intrinsics |
| | tempfile.py | Full | 14.2KB |
| | shutil.py | Partial | 9.4KB |
| | glob.py | Full | 3.7KB |
| | fnmatch.py | Full | 1.9KB |
| | zipfile.py | Partial | 3.6KB wrapper; core CRC32 via intrinsic; unsupported compression methods raise NotImplementedError |
| | tarfile.py | Partial | 3.6KB |
| | gzip.py | Partial | 4.8KB |
| | bz2.py | Partial | 6.2KB |
| | lzma.py | Partial | 9.0KB |
| **Time/Calendar** | datetime.py | Partial | 33.0KB; basic classes/methods OK; zoneinfo integration pending |
| | time.py | Partial | 11.7KB |
| | calendar.py | Partial | 7.1KB |
| | zoneinfo/ | Stub | _zoneinfo.py/tzpath.py/common.py all marked TODO(stdlib-parity, milestone:SL3, status:planned) |
| **Data Serialization** | json.py | Partial | 16.4KB; pure-Python codec; runtime fast-path + cls/callback parity pending |
| | pickle.py | Partial | 8.5KB wrapper; intrinsic-backed core dumps/loads (protocol 0-5, memo, reducer, state_setter, PickleBuffer); differential green 10/10 |
| | marshal.py | Partial | 0.6KB |
| | codecs.py | Full | 16.4KB |
| | base64.py | Full | 5.9KB |
| | binascii.py | Partial | 1.2KB |
| | quopri.py | Full | 4.1KB |
| | uu.py | Partial | 0.6KB |
| | tomllib/ | Partial | _parser.py marked TODO(stdlib-parity, milestone:SL3, status:planned); pure-Python parsing OK |
| **Crypto/Hash** | hashlib.py | Partial | 11.3KB; SHA/MD5/Blake2 OK via intrinsics; deterministic policy pending |
| | hmac.py | Partial | 2.9KB |
| | secrets.py | Full | 3.0KB |
| | ssl.py | Partial | 11.4KB; recv/send flags unsupported; resumption not yet supported |
| **Compression** | _compression.py | Full | 5.1KB |
| **Database** | sqlite3/ | Partial | _sqlite3.py 23.9KB; most ops OK; some edge cases pending |
| | shelve.py | Full | 5.7KB |
| | dbm/ | Stub | All marked TODO(stdlib-parity, milestone:SL3, status:planned) |
| **Network** | socket.py | Partial | 32.1KB; core ops OK via intrinsics; dir_fd variants unsupported; wasm missing threading; ancillary-data parity complete (msg_flags end-to-end) |
| | socketserver.py | Partial | 15.2KB |
| | select.py | Partial | 14.8KB; poll/epoll/kqueue/devpoll pending |
| | selectors.py | Partial | 12.3KB |
| | http/ | Partial | client/server stubs OK; advanced parity pending |
| | urllib/ | Partial | request/parse/error OK; advanced parity pending |
| | ftplib.py | Stub | 301B |
| | imaplib.py | Stub | 301B |
| | nntplib.py | Stub | 301B (TODO SL3) |
| | poplib.py | Stub | 301B |
| | smtplib.py | Partial | 6.4KB |
| | telnetlib.py | Partial | 0.7KB |
| **Async/Threading** | asyncio/ | Partial | Basic event loop/tasks/futures OK; runtime-heavy tranche blocked (4/5 fail; event-loop policy, table-ref trap, zipimport gap, thread unavailable in wasm) |
| | threading.py | Partial | 37.6KB; shared-memory semantics pending; full primitives pending |
| | concurrent/ | Partial | futures/thread pools OK; process pools pending |
| | _thread.py | Partial | 9.6KB; intrinsic-backed basic ops |
| | multiprocessing/ | Partial | fork/forkserver map to spawn (TODO RT3); Queue semantics divergent |
| **Logging** | logging/ | Partial | Logger/Handler/Formatter/LogRecord OK; percent-style intrinsic-backed; logging.config/handlers pending |
| **Testing** | unittest/ | Partial | runner/result/decorator parity pending expansion (TODO SL3) |
| | doctest.py | Not supported | eval/exec/compile policy-deferred (marked MOLT_COMPAT_ERROR in source) |
| **Debugging** | pdb.py | Stub | 301B |
| | bdb.py | Stub | 301B |
| | trace.py | Partial | 1.1KB |
| | traceback.py | Partial | 13.4KB; intrinsic-backed format_exception/extract_tb/exception-chain |
| | tracemalloc.py | Partial | 0.8KB |
| **Typing** | typing.py | Partial | 37.5KB; protocol/ABC bootstrap pending (fallback ABC scaffolding still present, marked TODO SL1) |
| | typing_extensions.py | Partial | 17.0KB |
| | dataclasses.py | Partial | 28.3KB; make_dataclass pending (dynamic class construction); inheritance from non-dataclass bases pending |
| **Enum** | enum.py | Partial | 11.0KB; aliases/functional API/Flag/IntFlag edge cases pending |
| **ABC** | abc.py | Partial | 6.0KB; intrinsic-backed boot (molt_classmethod_new, molt_staticmethod_new, molt_property_new); fallback scaffolding pending removal |
| **Context** | contextlib.py | Partial | 13.2KB |
| | contextvars.py | Partial | 4.0KB |
| **Utilities** | argparse.py | Partial | 12.1KB |
| | configparser.py | Partial | 16.5KB |
| | getopt.py | Partial | 6.9KB |
| | gettext.py | Partial | 20.4KB |
| | copy.py | Full | 2.2KB |
| | copyreg.py | Full | 3.3KB |
| | pprint.py | Full | 35.0KB |
| | reprlib.py | Full | 8.3KB |
| | linecache.py | Full | 9.7KB |
| **Misc System** | sys.py | Partial | 45.5KB; hexversion/api_version/abiflags/implementation via intrinsics; sys.flags intrinsic-backed |
| | site.py | Partial | 22.9KB |
| | sysconfig/ | Partial | |
| | platform.py | Partial | 3.0KB |
| | errno.py | Full | 1.0KB |
| | getpass.py | Stub | 301B |
| | grp.py | Stub | ~338B |
| | pwd.py | Stub | 301B |
| | spwd.py | Stub | 707B |
| **PEP695/701/709** | All 3.12+ syntax features | Supported | Type params, f-string improvements, comprehension inlining all tested in test_spec.py |

**Key Coverage Stats:**
- **Full implementations:** ~80 modules (100% API coverage)
- **Partial implementations:** ~140 modules (core ops OK, advanced features pending)
- **Stubs/Import-smoke:** ~60 modules (301B files; intrinsic readiness markers only)
- **Explicit NotImplementedError gaps:** 120+ locations (grep count)

### 1.2 Builtin Type Coverage

**File:** `/Users/adpena/Projects/molt/src/molt/stdlib/builtins.py` (22.7KB)

**Present:**
- Basic types: int, float, str, bytes, bytearray, list, dict, set, frozenset, tuple, range, slice
- Type constructors: bool, complex (pending), type, object
- Exceptions: all 3.12+ exception hierarchy
- Descriptors: classmethod, staticmethod, property (intrinsic-backed bootstrap via molt_bootstrap_descriptor_types)
- Iterator/generator protocol: iter, next, reversed, enumerate, zip, map, filter (pending full parity)
- Reflection: len, isinstance, issubclass, callable, hasattr, getattr, setattr, dir, vars, id, hash (partial)
- Formatting: format, repr (missing: complex repr; format spec edge cases)
- Numeric: abs, round, pow, divmod, min, max, sum (all present)
- I/O: input, print (present)
- Introspection: dir, globals, locals (intrinsic-backed)

**Known missing/stubbed:**
- **complex literals:** Pending complex type implementation (float-only for now)
- **eval/exec/compile:** Policy-deferred; compile validation-only (parser-backed code objects)
- **breakpoint:** Pending I/O builtin implementation
- **Exception.__traceback__:** Exception object model pending

### 1.3 Compliance Test Coverage

**Location:** `/Users/adpena/Projects/molt/tests/compliance/py312/test_spec.py` (7.3KB)

**Test coverage:**
- PEP 695 (type params): 3 tests (simple alias, generic alias, generic function)
- PEP 701 (f-strings): 4 tests (basic, expression, nested quotes, format spec)
- PEP 709 (comprehension inlining): 5 tests (list, list+filter, dict, set, nested)
- Process exit/atexit: 1 test
- math.sumprod (3.12 new): 2 tests
- Native scalar lane correctness: 3 tests (large int bitwise, arithmetic, shift semantics)
- importlib package state ownership: 1 test

**Total compliance tests:** ~19 explicit 3.12-specific tests

**Gap:** No comprehensive stdlib surface matrix test (missing coverage for os.walk, socket, asyncio, threading, etc. in formal compliance harness). Differential testing exists but is scattered across `tests/differential/stdlib/` rather than centralized in compliance matrix.

### 1.4 Known Coverage Gaps (Ranked by User Impact)

| Gap | Module | Severity | File:Line | Notes |
|-----|--------|----------|-----------|-------|
| dir_fd parameter unsupported | os | HIGH | /src/molt/stdlib/os.py:~1387-1410 | 12x NotImplementedError (readlink, symlink, stat, lstat, rename src/dst, replace src/dst, link src/dst, utime) — breaks POSIX-compatible code |
| os.walk returns eager list (OOM hazard) | os | HIGH | /src/molt/stdlib/os.py:1427-1439 | Intrinsic returns pre-materialized list; no generator semantics; known blocker for large directory traversal |
| Generator/iterator protocol incomplete | builtins, itertools | HIGH | scattered | yield, yield from, async generators pending; limited to narrow LIST_FROM_RANGE fast path |
| Threading semantics divergent | multiprocessing._core | MEDIUM | /src/molt/stdlib/multiprocessing/_core.py:~67-68 | fork/forkserver map to spawn; Queue.put/get raise NotImplementedError for parent/child|
| SSL recv/send/sendall flags | ssl.py | MEDIUM | /src/molt/stdlib/ssl.py:~1100, ~1150, ~1200 | NotImplementedError on MSG_* flags (MSG_PEEK, MSG_OOB, etc.) |
| Regex advanced features | re.py | MEDIUM | native engine | Lookahead, named groups, flag scoping, backreferences all raise NotImplementedError; host fallback disabled |
| doctest eval/exec blocked | doctest.py | MEDIUM | /src/molt/stdlib/doctest.py:~260 | MOLT_COMPAT_ERROR: eval/exec/compile unsupported; module unusable |
| asyncio runtime-heavy wasm blocked | asyncio/ | MEDIUM | /docs/ROADMAP.md:171 | 4/5 runtime-heavy tests fail; event-loop policy, table-ref trap, zipimport gap, thread unavailable |
| zoneinfo stubs not implemented | zoneinfo/ | LOW-MEDIUM | /src/molt/stdlib/zoneinfo/{_zoneinfo,_tzpath,_common}.py | All marked TODO(SL3, planned); currently import-smoke stubs |
| socket ancillary data (partial) | socket.py, _socket.py | LOW | /src/molt/stdlib/_socket.py:~450 | wasm streams OK; non-Unix routes return EOPNOTSUPP for non-empty control messages |
| Compression methods unsupported | zipfile.py | LOW | /src/molt/stdlib/zipfile/__init__.py:~1100, ~1200 | Some compression formats raise NotImplementedError (zstandard, etc.) |

---

## LANE 2: FRONTIER STDLIB STATE (RUST INTRINSICS VS PURE PYTHON)

### 2.1 Intrinsics Boundary Map

**Intrinsics registry:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/intrinsics/generated.rs` (24,429 lines; auto-generated)

**Python intrinsics interface:** `/Users/adpena/Projects/molt/src/molt/stdlib/_intrinsics.py` (5.8KB; resolution helpers)

**Usage density:** 2,453 `_require_intrinsic` / `_require_callable_intrinsic` calls across stdlib (grep count)

**Native Rust intrinsics (high leverage):**
- **Object model:** molt_globals_builtin, molt_locals_builtin, molt_module_import, molt_bootstrap_descriptor_types
- **Path/FS:** molt_os_listdir, molt_os_walk (eager list), molt_os_stat, molt_os_lstat, molt_os_fstat, molt_os_rename, molt_os_replace, molt_os_getcwd, molt_path_mkdir, molt_path_makedirs
- **Math/numerics:** molt_math_sumprod, math floor/ceil/trunc intrinsics
- **Pickle:** molt_pickle_dumps_core, molt_pickle_loads_core (protocols 0-5, memo, reducers, state_setter, PickleBuffer NEXT_BUFFER/READONLY_BUFFER)
- **JSON:** molt_json_encode, molt_json_decode (fast-path; Python fallback for edge cases)
- **Logging:** molt_logging_percent_style_format
- **Zipfile:** molt_zipfile_crc32
- **Socket:** molt_socket_*, molt_socket_constants
- **Importlib:** molt_module_import, molt_importlib_find_spec_orchestrate, molt_importlib_metadata_*, molt_importlib_resources_*, molt_importlib_exec_restricted_source
- **Sys metadata:** molt_sys_hexversion, molt_sys_api_version, molt_sys_abiflags, molt_sys_implementation_payload, molt_sys_flags_payload
- **Weakref:** molt_weakref_callback
- **Traceback:** molt_traceback_format_exception, molt_traceback_extract_tb, molt_traceback_exception_chain_payload, molt_traceback_exception_suppress_context
- **GPU:** molt_gpu_* (tensor, buffer, scheduler ops — see LANE 3)
- **Env:** molt_env_snapshot, molt_env_set, molt_env_unset, molt_env_putenv, molt_env_unsetenv
- **Time:** molt_time_* (from runtime/molt-runtime/src/builtins/time.rs)
- **Process/OS:** molt_getpid, molt_getuid, molt_uname, molt_cpu_count, molt_getlogin
- **Collections:** molt_bisect_* (insort, bisect_left/right)

**Pure Python modules (no intrinsic lowering):**
- `itertools.py` (5.5KB) — pure Python, fuses but no scheduler generator protocol
- `functools.py` (9.8KB) — pure Python, lru_cache not yet decorator-lowered
- `copy.py`, `copyreg.py`, `linecache.py`, `pprint.py`, `reprlib.py` — all pure Python
- Most of `collections/`, `dataclasses.py`, `enum.py` — pure Python base classes with some edge cases pending
- Full `typing.py` (37.5KB) — pure Python runtime; bootstrap scaffolding pending removal

### 2.2 Known-Stubbed/Broken Stdlib (Top Offenders)

**Top 20 stubbed modules (301B import-smoke marker files):**

1. readline.py — interactive shell history
2. rlcompleter.py — REPL autocomplete
3. webbrowser.py — browser launching
4. pdb.py, bdb.py — debuggers
5. tty.py — terminal control
6. wave.py — audio format I/O
7. netrc.py — netrc auth file parsing
8. modulefinder.py — import graph analysis
9. fileinput.py — line-by-line file iteration
10. resource.py — Unix resource limits
11. pydoc.py — documentation browser
12. posixpath.py, genericpath.py — OS-specific path math (pure stubs; pathlib.py is full)
13. ftplib.py, imaplib.py, nntplib.py, poplib.py — legacy network protocols
14. pwd.py, grp.py, spwd.py — Unix user/group databases
15. plistlib.py — Apple property list format
16. optparse.py — deprecated argparse predecessor
17. mmap.py — memory-mapped file I/O
18. pty.py — pseudo-terminal I/O
19. fcntl.py — file control (partial; ioctl/flock/lockf raise OSError)
20. encodings/_win_cp_codecs.py — Windows code page encodings (TODO SL3)

**Reasons for stubbing:**
- **Interactive/REPL-only:** readline, rlcompleter, pdb, pydoc
- **Legacy/deprecated:** optparse, imaplib, poplib, ftplib, nntplib
- **Unix-specific with no WASM translation:** pwd, grp, spwd, resource, pty, tty
- **Binary format/external tools:** wave, plistlib, mmap
- **System-level access pending:** fcntl (ioctl/flock), mmap
- **Win32-specific:** encodings/_win_cp_codecs

**NotImplementedError hotspots (grouped by module):**

| Module | Count | Examples |
|--------|-------|----------|
| os.py | 12 | dir_fd variants; utime(ns=...) |
| numbers.py | 13 | All abstract methods (by design) |
| ssl.py | 3 | recv/send/sendall flags |
| zipfile.py | 3 | Unsupported compression methods |
| _socket.py | scattered | Not implemented for this target |
| multiprocessing | 2 | Queue.put/get parent/child semantics |

### 2.3 os.walk OOM/Iterator Hazard

**File:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/os_ext.rs:218-358`

**Root cause:** Eager list materialization in `walk_dir_collect()` — `molt_os_walk` collects the ENTIRE directory tree into a Rust Vec, allocates Python tuples/lists/strings for every entry, and returns one massive list. The Python wrapper at `/src/molt/stdlib/os.py:1427-1439` `yield`s from the already-materialized intrinsic result, so lazy iteration in user code does not bound memory.

**Known blockers:** Generator protocol lowering not yet complete (generator state in wasm ABI remains TODO TC2). os.walk rewrite awaits generator fusion (the keystone arc — see [[project_generator_fusion_keystone]] memory + docs/design/generator_fusion.md).

---

## LANE 3: GPU SUBSYSTEM (MOLT.GPU, TINYGRAD FIDELITY, DFLASH)

### 3.1 GPU Architecture Map

**Location:** `/Users/adpena/Projects/molt/runtime/molt-gpu/` (Rust implementation)
**Python API:** `/Users/adpena/Projects/molt/src/molt/gpu/` (tensor.py, ops.py, etc.)
**Tinygrad compat:** `/Users/adpena/Projects/molt/src/tinygrad/` (thin wrapper; delegates to `molt.gpu.Tensor` for module and from-import public API, with exact-case import graph custody)

| Component | File | Status | Notes |
|-----------|------|--------|-------|
| **LazyOp DAG** | lazy.rs (5.6KB) | Done | Deferred computation graph; Buffer/Unary/Cast/Binary/Ternary/Reduce/Movement/Contiguous nodes; cast target dtype is explicit |
| **Scheduler** | schedule.rs (21.6KB) | Done | DAG → topological FusedKernel list; backend-adaptive workgroup sizing (Vulkan 256, Metal 128, D3D12 256, GL 64) |
| **Fusion Engine** | fuse.rs (17.7KB) | Done | Merge consecutive elementwise; elementwise→reduce→elementwise chains; reduce-to-reduce is fusion boundary |
| **ShapeTracker** | shapetracker.rs (14.8KB) | Done | Zero-copy view system (shape, strides, offset, optional mask); all movement ops O(1); shrink now rewrites padded mask coordinates before composition |
| **Op Definitions** | ops.rs (4.9KB) | Done | 26 primitive ops (9 unary, 14 binary, 1 ternary, 2 reduce, 6 movement) |
| **DType System** | dtype.rs (11.4KB) | Done | f32, f16, bf16, i32, i64, u32, u64, bool; narrowing at render time (e.g., f64→f32 on Metal) |
| **DCE** | dce.rs (5.7KB) | Done | Removes unused nodes from DAG before scheduling |
| **MLIR/MIL Codegen** | mlir.rs, render/mil.rs | Partial/active | Cross-compilation targets; MLIR `MaterializeCopy` and pure elementwise compute emit real flat-memref `scf.for` lowering with ShapeTracker index/mask support; non-MXFP casts use explicit `arith` conversion selection from first-class lazy/scheduler target dtype; MIL `MaterializeCopy` has verified Bool/Int8/16/32/UInt8/16/32/Float16/Float32 gather/select lowering; reductions, MXFP quantized casts, and MIL storage lanes without Core ML package compile/run/raw-byte proof remain fail-closed |
| **CPU Executor** | device/cpu.rs (63.5KB) | Done | Reference implementation; ShapeTracker-aware raw materialization plus typed scalar Cast/Bitcast execution for terminal, fused intermediate, and pre-reduce values; runtime raw readback exposes dtype, storage byte count, and exact byte copy while legacy f32 readback rejects non-Float32 |
| **WASM CPU** | device/wasm_cpu.rs (6.3KB) | Done | Browser fallback using wasm32 bounds checks |
| **Metal** | device/metal.rs (9.3KB) + render/msl.rs (17.4KB) + render/msl4.rs (19.0KB) | Done | MSL codegen; f32, f16, bf16, i32, i64 (no f64); raw byte proof covers non-f32 Cast/Bitcast storage for Float32->Int32/UInt16/UInt8 and equal-width Float32<->UInt32 |
| **WebGPU** | device/webgpu.rs (18.5KB) + render/wgsl.rs (18.6KB) | Done | WGSL codegen; f32, f16, i32, u32 (no f64, i64, u64) |
| **WebGL2** | device/webgl2.rs (13.7KB) + render/glsl.rs (18.9KB) | Done | GLSL ES 3.0; f32, i32, u32 only |
| **CUDA** | render/cuda.rs (17.3KB) | Done | CUDA C codegen; full dtype support incl. bf16 via nv_bfloat16 |
| **HIP** | render/hip.rs (15.9KB) | Done | HIP C codegen; full dtype support via hip_bfloat16 |
| **OpenCL** | render/opencl.rs (20.2KB) + device/opencl.rs (5.5KB) | Done | f64 via cl_khr_fp64, i64 native, no bf16 |
| **Apple Neural Engine** | device/ane.rs (13.8KB) | Experimental | ANE execution; Apple-specific quantized inference |
| **Memory Arena** | arena.rs (10.3KB) | Done | Pool allocator for GPU buffers |

All layers green under test: test_schedule_spec.rs, test_fusion.rs, test_constant_fold.rs, test_ops.rs, test_shapetracker.rs, test_render_{msl,wgsl,glsl,cuda,hip,opencl}.rs, test_inference_pipeline.rs, test_e2e_compositions.rs, test_stress.rs, test_concurrency.rs, test_wasm_compat.rs.

### 3.2 Python Tensor API Surface (Tinygrad Fidelity)

**File:** `/Users/adpena/Projects/molt/src/molt/gpu/tensor.py` (90.2KB) — 80+ Tensor methods: creation (zeros/ones/full/eye/arange/linspace/normal/uniform/stack), shape ops, fancy indexing, arithmetic/comparison/bitwise, reductions (sum/mean/var/std/min/max/argmin/argmax), matmul (RESHAPE+EXPAND+MUL+REDUCE_SUM composition), activations (relu/sigmoid/tanh/softmax/log_softmax/gelu/silu), norms (layernorm/batchnorm/rmsnorm), scaled_dot_product_attention, conv2d/conv_transpose2d (im2col composition), pooling, 4-bit TurboQuant dequant, cat/split/chunk/flatten, KV-cache ops (take_rows, scatter_rows, linear_split_last_dim, scaled_relu_gate_interleaved).

**Stdlib tinygrad wrapper:** `/Users/adpena/Projects/molt/src/molt/stdlib/tinygrad/tensor.py` now routes typed constructors, zeros, raw readback, unary/binary/ternary `where`/cast, explicit-axis reductions, Rust-owned all-axis reductions via `molt_gpu_prim_reduce_all`, movement-family views (`reshape`, `expand`, `permute`, zero-fill `pad`, `shrink`, `flip`, `contiguous`), and matmul composition through runtime GPU primitive handles. The public `src/tinygrad/` shim preserves `molt.gpu.Tensor` for `import tinygrad` and `from tinygrad import Tensor`, with `where_promotion` and `movement_views` in the off-the-shelf adapter as the current dtype/ternary and movement compatibility workloads. Remaining wrapper migration lanes are convolution, which needs a first-class window/im2col view primitive, and nonzero-pad semantics, which remain fail-closed until typed pad-fill or mask/`where` behavior is defined across runtime and backends.

Falcon-OCR VLM e2e working (DFlash multi-head attention, RMSNorm, rotary embeddings, patch embeddings); quantized inference working.

**Known gaps vs full tinygrad:**

| Feature | Molt Status | tinygrad Status | Notes |
|---------|-------------|-----------------|-------|
| Autograd | Not implemented | Full AD | Molt is inference-only today; training = open frontier |
| Dynamic shapes | Eager eval only | Symbolic shapes | Compile-time shape materialization via intrinsics |
| Complex dtypes | Not implemented | Supported | Backends lack complex support |
| Sparse tensors | Not implemented | Limited support | Not prioritized for inference |
| Custom kernels | Not implemented | Via jit() | User kernels via render overrides (advanced) |

**API compatibility claim:** "Same 3 OpTypes and 26 ops that tinygrad uses." — VERIFIED (ops.rs OpType enum matches tinygrad's UnaryOp/BinaryOp/ReduceOp/MovementOp taxonomy).

### 3.3 DFlash Contract & Implementation State

**Design doc:** `/Users/adpena/Projects/molt/docs/spec/areas/perf/0520_DFLASH_CONTRACT.md`
**Implementation:** `/Users/adpena/Projects/molt/src/molt/gpu/dflash/` plus
generic speculative loop primitives in `/Users/adpena/Projects/molt/src/molt/gpu/speculative.py`

| Component | File | Status |
|-----------|------|--------|
| Generic speculative protocol | ../speculative.py | Complete for neutral lossless block-speculative request/result/loop primitives; not a DFlash claim |
| DFlash contracts | contracts.py | Complete fail-closed contract specialization: DFlashConditioning, DFlashRuntime, and refresh validation require target_features, target_kv, position_ids, last_verified_token |
| Adapters | adapters.py (5.3KB) | Complete — registry; resolve_dflash_adapter fail-closed on missing |
| DFlash package runtime bridge | deleted | Removed; generic loops no longer live under the DFlash namespace |
| KV Cache | ../kv_cache.py (31.2KB) | Complete — tiered, H2O importance scoring, eviction, DDTree-style block-diagonal layout |
| Generate | ../generate.py (8.0KB) | Routes plain greedy, generic block-speculative callbacks, and fail-closed DFlash adapter resolution |

**Paper-required properties:** fail-closed contract properties are verified:
target-conditioned payload fields, verifier/drafter separation, position IDs,
last verified token type checks, typed adapter metadata, and fail-closed adapter
resolution. Algorithmic properties are **not** complete until a real
target-conditioned block-diffusion drafter consumes target hidden features,
performs per-layer KV injection, and passes the reference losslessness oracle.

**Verification status:** PASS for the fail-closed DFlash namespace/adapter
contract; PENDING for F1/F2/F4/F5 algorithmic DFlash fidelity.

**Known limitations (documented):** no trained drafter shipped (contract only; drafter models require external training); block size configurable but not auto-tuned.

---

## RANKED TOP-10 GAPS (BY USER-VISIBLE PARITY × FOUNDATION CRITICALITY)

| Rank | Gap | Impact | Foundation | Effort | File:Line | Status |
|------|-----|--------|-----------|--------|-----------|--------|
| **1** | os.walk returns eager list (OOM hazard) | CRITICAL | Iterator protocol, generator semantics | High | /runtime/molt-runtime/src/builtins/os_ext.rs:218-358 | Blocked on generator fusion (the keystone arc) |
| **2** | Generator/async generator protocol incomplete | CRITICAL | Language semantics (yield, yield from, async gen) | Very High | scattered + lowering | Blocked (TC2 pending; CoroElide/generator-fusion = Tier-3 of foundation program) |
| **3** | dir_fd parameter unsupported across os.* | HIGH | POSIX compliance | Medium | /src/molt/stdlib/os.py:1387-1410 | Needs intrinsic variants design |
| **4** | asyncio runtime-heavy wasm blocked (4/5 fail) | HIGH | Async correctness in browser | High | ROADMAP.md:171 claims | Event-loop policy, table-ref trap, zipimport gap, no threads |
| **5** | Regex advanced features (lookahead, named groups, backrefs) | MEDIUM | Text processing parity | Medium | native regex engine | NotImplementedError; host fallback disabled |
| **6** | Socket/SSL MSG_* flags unsupported | MEDIUM | Network edge cases | Low-Medium | ssl.py ~1100-1200 | Incremental |
| **7** | doctest eval/exec blocked | MEDIUM | Testing framework | Low (policy) | doctest.py ~260 | Policy-deferred by design (no dynamic exec) |
| **8** | zoneinfo/datetime timezone stubs | MEDIUM | Time correctness | High | zoneinfo/* | TODO(SL3, planned) |
| **9** | multiprocessing fork/forkserver → spawn | MEDIUM | Process model | High | multiprocessing/_core.py:67-68 | Needs decision doc: spawn-only canonical? |
| **10** | dataclasses.make_dataclass + non-dataclass bases | LOW-MEDIUM | Dataclass completeness | Medium | dataclasses.py | Blocked on dynamic class construction policy |

---

## DESIGN COVERAGE ANALYSIS

**Gaps WITH committed design docs:** GPU primitive stack (docs/architecture/gpu-primitive-stack.md), DFlash contract (docs/spec/areas/perf/0520_DFLASH_CONTRACT.md), generator fusion (docs/design/generator_fusion.md — covers rank-1/2), compiler foundation program (docs/design/compiler_foundation_gap_analysis.md).

**Gaps with NO design coverage (highest risk):**
1. **asyncio wasm event-loop policy** — 4/5 failing with no root-cause design doc
2. **dir_fd intrinsic variants** — needs design for intrinsic-backed variant semantics
3. **Regex feature-expansion strategy** — no milestone plan for lookahead/named groups/flag scoping
4. **Threading semantics divergence** — no formal spec of supported vs divergent operations
5. **Socket ancillary-data residual cases** — partial implementation, no enumeration of what remains

## SUMMARY STATISTICS

- 280+ stdlib modules present: ~80 full / ~140 partial / ~60 stubs; 120+ NotImplementedError sites
- 4 CRITICAL blockers: os.walk OOM, generator protocol, asyncio-wasm, dir_fd
- Compliance harness covers only ~19 explicit 3.12 tests — no comprehensive surface matrix
- GPU: 26 primitives × 8 backends green; tinygrad taxonomy verified; DFlash contract paper-faithful (no trained drafter shipped — correctly not faked)
