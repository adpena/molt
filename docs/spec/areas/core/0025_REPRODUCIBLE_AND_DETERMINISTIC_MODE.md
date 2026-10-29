# Reproducible And Deterministic Mode
**Spec ID:** 0025
**Status:** Draft
**Priority:** P1
**Audience:** compiler engineers, runtime engineers, tooling engineers
**Goal:** Define deterministic build/runtime behavior and reproducibility rules.

---

## 1. Definitions
- **Reproducible build**: identical binary given identical inputs and toolchain.
- **Deterministic runtime**: identical observable behavior given identical inputs.

---

## 2. Build Determinism
Builds must be reproducible when the deterministic flag is enabled.

### 2.1 Inputs That Must Be Stable
- Source tree content and ordering.
- Lockfiles (`uv.lock`, Cargo.lock).
- Toolchain versions (Rust, Python, linker).

### 2.2 Build Rules
- No nondeterministic timestamps embedded in artifacts.
- Stable ordering for any generated tables or metadata.
- Compiler emission order and tie-breaking must be independent of internal hash seeds.

Compiler lookup tables may use randomized hashing. Select their representation
for the access pattern and measured cost; preserve required source ordering and
canonicalize output ordering at the boundaries that need it. Neither container
lifetime nor `HashMap`/`BTreeMap` choice proves determinism. Python-visible
collection ordering and runtime hashing follow their language and runtime
contracts.

Qualify these properties with the existing
[production IR hash-seed comparisons](../../../../tests/determinism/test_ir_determinism.py),
[repeated artifact comparisons](../../../../tools/check_reproducible_build.py), and
[runtime observation comparisons](../../../../tools/check_deterministic_runtime.py).
These are development and release checks, with no added work in the end-user
binary. Evidence establishes only the exercised cells; see section 5.1.

Release product reproducibility compares independently built final bytes and
canonical product manifests. Private build observations remain exact and may
contain different physical paths. They are preserved in build evidence, never
normalized away to establish source-cache reuse. Native and WASM use separate
strict source-envelope and installed-product readers; the
[Cargo input contract](../../../design/cargo_input_authority.md#observed-environment-inputs)
owns their schemas and release evidence custody.

### 2.3 Build-path independence

The location of the compiler's sources is not a build input. The same compiler
sources build the same objects from any checkout, worktree or installed
distribution (HF-372). Before this rule, every native object embedded the
checkout path of each stdlib module it compiled, for example
`/Users/x/Molt/worktrees/jitp1/src/molt/stdlib/sys.py`. The same source built in
two worktrees gave 12 of 12 different objects, so no cache or identity check
could match them across checkouts.

One authority, `src/molt/logical_source_paths.py`, gives each module its
logical source path: the one path that its compiled code reports. The CLI
assigns it once per module graph.

| Module | Reported path | Model |
| --- | --- | --- |
| Program module | the absolute file it was compiled from | CPython: `__main__.__file__` is absolute since 3.9, and imported modules report the file the path finder found |
| Toolchain-owned module: a file of the compiler's own `molt` package (the bundled stdlib, `molt.gpu`, `molt.lib`, `molt`) | `/__molt__/<path in the package>`, for example `/__molt__/stdlib/json/__init__.py` | rustc's std paths `/rustc/<commit>/library/...`; Go `-trimpath` |
| Namespace package | no source origin; its typed registry row carries ordered directory portions | no generated source or body; the spec origin is `None` |

Every emitted module location is a projection of that path: `__file__`,
`__spec__.origin`, the package `__path__`, the module and function
`co_filename`, `FunctionIR.source_file` (cache keys, stdlib batch grouping,
fact-graph source sites), deferred compile warnings, the module registry origin
(`runpy`), and the traceback source registry in the native entry stub. That
registry lists only files that some module reports as its own path. Stdlib
type-hint trust and the stdlib midend policy select on the logical stdlib
prefix, not on a path spelling.

How other toolchains solve it:

- rustc `--remap-path-prefix=FROM=TO` rewrites paths in debug info, diagnostics
  and `file!()`. Rust's release toolchains remap the std sources to
  `/rustc/<commit-hash>` (bootstrap `rust.remap-debuginfo`); a debugger maps
  that prefix back to the installed `rust-src` component with
  `set substitute-path`
  ([rustc book](https://doc.rust-lang.org/rustc/command-line-arguments.html#--remap-path-prefix-remap-source-names-in-output)).
  Cargo's `trim-paths` ([RFC 3127](https://rust-lang.github.io/rfcs/3127-trim-paths.html))
  makes workspace paths relative, names dependencies `<name>-<version>` and
  separates the object scope from the diagnostics scope.
- GCC and Clang `-ffile-prefix-map=OLD=NEW` covers `__FILE__` and DWARF
  (`-fmacro-prefix-map` and `-fdebug-prefix-map`); MSVC needs `/pathmap` with
  `/experimental:deterministic` (HF-363).
- Go `-trimpath` records `module@version/file.go` for modules and the import
  path for the standard library ([cmd/go](https://pkg.go.dev/cmd/go#hdr-Compile_packages_and_dependencies)).
- CPython gives a frozen module `co_filename` `<frozen NAME>` and computes its
  `__file__` at startup from `sys._stdlib_dir` (3.11+). Its tracebacks show no
  source line for such a frame (3.12 to 3.14); 3.14's `linecache` can map a
  frozen filename back through `module_globals["__file__"]`. A cached code
  object gets the real import location through `_imp._fix_co_filename`.

Molt's choices, and why:

- The stdlib keeps a path-shaped name, not `<frozen NAME>`. Molt compiles every
  stdlib module ahead of time, not only CPython's frozen set. One rule keeps
  `co_filename == __file__ == __spec__.origin`, as for CPython's non-frozen
  stdlib, keeps basenames and package-relative tails, and keeps
  `os.path.dirname(__file__)` meaningful.
- The prefix carries no version. The build already records the compiler
  identity, and every stdlib object key binds it, so a version in every path
  would add bytes and no information. Map a path back by replacing `/__molt__/`
  with `<compiler source root>/src/molt/`.
- A program module keeps its absolute path for CPython parity and for
  `Path(__file__).parent` data lookups. A program inside a checkout, such as the
  differential corpus, therefore still builds checkout-specific program
  objects; its stdlib objects are identical. Relocatable program objects, with
  the path bound at link time as `_fix_co_filename` binds it at import, are not
  implemented.
- A traceback frame or compile warning of a toolchain module shows no source
  line, as CPython shows none for a frozen module's frame. A shipped program
  must not read files from the machine that built it. For the same reason,
  `importlib.resources` cannot reach a toolchain package's files through its
  `__file__`; serving them needs the compiled image to carry them.
- The native backend emits no DWARF or CodeView yet. When it does, its file
  names must come from these logical paths. The Rust runtime and backend builds
  remap their own source paths separately in release packaging
  (`tools/release/native_build.py`, HF-363). A development backend executable
  still embeds its Cargo target directory, which names the checkout, through
  the generated sources of `cranelift-codegen`. Two checkouts therefore run
  different backend files that emit identical objects, and a cache key that
  binds the backend file's digest (native stdlib constituent objects) still
  differs between them.

Cost: one `Path.parts` prefix comparison per module per module graph, with no
system call; the compiler package root resolves once per graph. The entry stub
no longer embeds a string for each stdlib file.

Proof: `tests/cli/test_logical_source_paths.py` checks every projection from two
compiler checkouts; `tests/differential/stdlib/module_source_paths.py` checks the
CPython-visible shape; `tests/cli/test_checkout_independent_objects.py` builds the
same programs from two compiler checkouts and compares the native object bytes.

---

## 3. Runtime Determinism

### 3.1 Time
- In deterministic mode, `time.time()` and `time.monotonic()` return a
  deterministic clock anchored to process start.
- Wall-clock access requires explicit capability grants.

### 3.2 Randomness
- `random` and any internal RNG must use a fixed seed by default.
- Explicit seeds override the default but remain deterministic.

### 3.3 Hashing
- Hash randomization is disabled or fixed to a stable seed.
- Hash results must be stable across runs and targets.

### 3.4 Scheduling
- Task scheduling is deterministic for identical workloads.
- Any non-deterministic scheduling policy must be explicitly gated.

---

## 4. Interfaces

### 4.1 Build Flag
- CLI: `molt build --deterministic`
- Environment: `MOLT_DETERMINISTIC=1`

### 4.2 Capability Gates
- `time.wall`: allow wall-clock access.
- `rand.nondeterministic`: allow nondeterministic RNG.

### 4.3 WASM Runtime Determinism

When `MOLT_DETERMINISTIC=1` is set, the WASM host (`molt-wasm-host`) automatically applies:

- **NaN canonicalization**: All NaN payloads are normalized to a canonical form via
  `cranelift_nan_canonicalization(true)`. This prevents CPU-specific NaN payload
  differences from producing divergent WASM execution results.

- **Sequential compilation**: `parallel_compilation(false)` ensures the Cranelift
  JIT produces identical native code regardless of thread scheduling during
  compilation.

These flags are applied automatically — no manual Node.js/wasmtime flags are needed
when deterministic mode is on.

#### Limitations

- WASM execution under V8 (Node.js) may still require `--no-wasm-tier-up` and
  `--liftoff-only` flags for full determinism. These are NOT auto-applied by Molt
  and must be passed explicitly when using Node.js as the WASM runner.
- Wasmtime's own tier-up (if enabled) should be disabled separately via
  `config.strategy(Strategy::Cranelift)` (already the default).

---

## 5. Validation
- Deterministic builds must be bit-identical.
- Deterministic runtime tests must repeat with stable outputs.
- WASM and native targets must match in deterministic mode.


### 5.1 Phase-specific proof evidence

`tools/check_deterministic_runtime.py` emits
`molt.deterministic-runtime-proof.v3`. Each observation records separate `build`
and `runtime` launch evidence. The compiler receives `PYTHONHASHSEED=0`; the
runtime receives the observation index (starting at 1). A report must record
those distinct environments, not infer a runtime setting from the build mode.
The build's cache and artifact directories likewise do not describe the guest's
inherited environment.

`tools/check_reproducible_build.py` emits `molt.reproducibility-proof.v3`.
Repeated artifact comparisons retain each `build` launch, while IR audit
observations retain their `compiler` launch (hash seeds start at 0). Compare-only
mode reads existing artifacts and does not manufacture compiler launch evidence.

The two tools project the same finite public fields from the environment passed
to each guarded command: `PYTHONPATH`, `PYTHONHASHSEED`, `MOLT_DETERMINISTIC`,
`MOLT_CACHE`, `MOLT_EXT_ROOT`, `MOLT_TARGET_ROOT`, `CARGO_TARGET_DIR`,
`MOLT_BACKEND_DAEMON`, `MOLT_BACKEND_DAEMON_SOCKET_DIR`, `TMP`, and `TEMP`.
Unset values are JSON `null`. These fields describe launch inputs, not the full
ambient environment or the program's later environment. Other inherited values
and credentials are not copied into the report.

Runtime observations share the inherited compiler build roots
(`CARGO_TARGET_DIR`, `MOLT_EXT_ROOT`, `MOLT_TARGET_ROOT`). Each observation
gets its own working directory, `MOLT_CACHE`, output path and no backend
daemon, so no compile state passes between observations. Each build uses the
proof plan's nested build budget unless `--build-timeout` sets one.

Launch evidence binds those fields to argv, cwd, guard return status, and child
return status. `completed` means that the guard returned without a recorded
guard interruption; a nonzero child return still fails the proof. `timeout`,
`guard-error`, and `error` are not successful observations. Exception evidence
records an attempted command and has no claimed child return status. A failed
build leaves `runtime` null. Completed earlier observations remain available if
a later phase fails; selected/executed/pass/fail/error counters still count
complete comparison cells, not subprocess attempts. Each cell also reports
`completed_runs`: observations that completed acquisition and temporary-resource
cleanup. Isolated build, runtime and IR directories use the shared owned-directory
allocator, which returns canonical host paths and refuses cleanup of a replaced
allocation. A failure during artifact hashing, stat, read, or cleanup retains prior
observations and all known launch records, but the affected cell is an error.
A completed child remains `completed` when a later parent-side operation fails;
setup failure before launch leaves the phase null. IR success requires exactly
the requested number of completed observations, regardless of whether an error
has a nonempty diagnostic string.

Build output must be a JSON object with a nonempty string artifact path. Invalid
objects, envelopes, or path values are build errors with the completed launch
retained and no runtime launch. Compare-only rejects the same malformed inputs
as a counted error without inventing process evidence.

Source admission belongs to each selected build or IR cell. Batch scheduling
must not prefilter sources or repeat a filesystem sweep after collecting build
results. When IR auditing is requested, every selected source has an IR cell,
including unreadable or missing sources; those cells count as errors rather than
disappearing from the denominator. Without IR auditing, no IR source admission
runs. Compare-only handles JSON reads/decoding, artifact-path admission and
artifact reads within its single counted result boundary.

Both tools resolve corpus names through `config/reproducibility_corpus.toml`.
The existing `full` corpus contains exactly `examples/hello.py`,
`examples/simple_ret.py`, and `examples/sieve_bench.py`. Its scheduled native
`dev` proof cell is a bounded qualification of those programs, not the full
release support matrix, CPython conformance, or cross-backend acceptance. The
canonical proof plan and verified-subset/release authorities retain those
broader obligations.
