# Development JIT tier: run without building

Status: steps 1-3 and 5 implemented, 2026-10-21 (HF-371). Step 4 (lazy compilation) and
"Incremental patching" remain. Verified cell: macOS arm64.

## Problem

`molt run` builds an executable and then runs it. Every edit pays object emission, a
system link against the runtime archive, output publication and process start, after the
frontend and backend work. Measured costs on this repository's hosts:

- A warm build of a small program spends 14.7 s, of it 5.9 s in `backend_prepare`
  (HF-266) and 36 toolchain probe processes (HF-357).
- A genuinely new linked program processes the combined runtime and app module; the
  historical macOS dev hello module was 24 MB and its first build took 38.9-41.4 s, of it
  24.7-26.2 s linking (HF-26). A tiny WASM program once linked 379 MB of inputs (HF-263).
- A no-change rebuild is 0.155 s (HF-250); the edit path is the slow one.

Goose (aardappel/goose), a new language with a 30 kLOC compiler, compiles and runs a
program in-process through libtcc with no build step. Molt has a Cranelift backend and a
runtime written in Rust; it can do the same.

## Leading practice

- **rustc_codegen_cranelift JIT mode** (`cargo-clif jit`, `-Cllvm-args=jit-mode`) runs the
  crate from memory through `cranelift_jit`, with no object file. Dependencies must be
  dynamic libraries, and the driver resolves their symbols through a lookup function
  ([usage.md](https://rust.googlesource.com/rust/+/HEAD/compiler/rustc_codegen_cranelift/docs/usage.md),
  [jit.rs](https://rust.googlesource.com/rust/+/HEAD/compiler/rustc_codegen_cranelift/src/driver/jit.rs)).
  Today `run_jit` compiles every mono item before it calls `main`. Its earlier lazy mode
  redefined functions through `JITModule` hotswap; `cranelift-jit` 0.137.0 has no hotswap
  and no `prepare_for_function_redefine`, so that model no longer exists.
- **Goose** emits C and runs it through in-process TinyCC; its test suite diffs the
  TinyCC run against the native C build as a third, independent C implementation
  ([docs/testing.md](https://github.com/aardappel/goose/blob/master/docs/testing.md)).
- **Julia** (LLVM ORC JIT) and **Swift's REPL** (LLDB + JIT) show the same split: one
  code generator, two code owners (memory for development, objects for shipping).
- **Zig's** in-place binary patching targets sub-millisecond incremental rebuilds but is
  not yet shipped as a stable feature
  ([kristoff.it](https://kristoff.it/blog/zig-new-relationship-llvm/)); it is not a model
  yet.

## Design

One code generator, two code owners. The native backend's Cranelift lowering builds every
function into a `cranelift_module::Module`; only the module that owns the code differs.

1. **Generic module** (7c86a042a). The function compiler takes
   `&mut dyn cranelift_module::Module`, as cg_clif's does. `SimpleBackend` holds a
   `CodeOwner`: `Object` (an `ObjectModule`, for the system linker) or `ProcessMemory`
   (a `JITModule`, feature `jit`). `CodeOwner` implements `Module` by forwarding every
   method, as cranelift_module's own `impl Module for &mut M` does. One ISA constructor
   (`native_isa`) serves both owners; only the relocation model follows `CodePlacement`.
   Process memory uses `is_pic` on x86_64 only and long-range libcalls, as
   `JITBuilder::with_flags` does, and one 1 GiB `ArenaMemoryProvider` reservation keeps
   generated code and data within PC-relative range. Object bytes stay identical: the base
   and changed release backends emit the same object and archive for the hello program.
2. **Runner.** `molt-jit` (`runtime/molt-jit`) embeds the native code generator and
   `cranelift_jit`. It builds as `HostToolProduct.JIT` through the backend binary's Cargo
   plan, source receipts, generation store and capability probe, with the backend's Cargo
   profile and CPU policy. Its probe compiles and runs one function, so a host whose W^X
   policy refuses JIT code fails admission. It reads a JIT image, compiles the program and
   starts it in process. Stdout, stderr, exit status and signals are the program's own.

   The program must start in the process state an AOT executable gives it, so the runner
   is a C `main` (`#![no_main]`), not a Rust `main`: Rust's start-up code sets `SIGPIPE`
   to `SIG_IGN` and installs `SIGSEGV`/`SIGBUS` handlers on alternate signal stacks, and a
   program would inherit both (a write to a closed pipe raised `BrokenPipeError` instead
   of ending the process with `SIGPIPE`; a native stack overflow aborted with a message
   instead of `SIGSEGV`). Compilation runs on its own thread with the compiler's 64 MiB
   stack (`COMPILE_THREAD_STACK_BYTES`), inside `catch_unwind`; the runner then joins it
   and retires every compiler pool thread (`retire_compile_thread_pools`), so the program
   starts with one thread, as a C `main` does.

   Compiler stderr appears only when compilation fails or backend diagnostics were
   requested, as `molt run` shows a backend's stderr. While it compiles, the runner points
   file descriptor 2 at a pipe that a forked keeper process holds in memory; the runner
   then tells the keeper to discard or to write the text. A runner that dies while it
   compiles, by any signal or exit, never gives a verdict, and the keeper writes the text.
   The keeper is reaped before the program starts.
3. **Runtime symbols.** Generated code imports runtime functions through
   `Linkage::Import`. The runner loads the runtime as a shared image and resolves every
   import with `dlsym` on that image, as cg_clif resolves dependency symbols in their
   dylibs. Every import must resolve before relocation; a missing one fails with its name
   before any code runs, as an AOT link fails. The ABI witness
   (`molt_generated_object_abi_<fingerprint>_<mode>`) is an import like any other, so a
   runtime built for the other execution mode is refused.

   The image is linked once per runtime generation from the exact AOT archive
   (`-force_load`, no dead stripping; `molt_isolate_bootstrap` stays undefined and binds
   to the runner's export). JIT and AOT runs therefore use the same runtime bytes, and the
   image defines exactly what an AOT link resolves against. It is content-addressed beside
   the generation's callable-symbol file. This replaces the earlier plan of a symbol table
   generated from the runtime-callable ABI manifest, for two measured reasons: the
   manifest projections (`runtime_callable_abi_generated.rs`,
   `runtime_boxed_abi_generated.rs`) omit 124 of the 359 `molt_*` helper names the
   Cranelift backend spells as literals, so a generated table cannot be complete; and a
   Rust executable dead-strips every unreferenced `#[no_mangle]` runtime function, so the
   runtime cannot be linked into the runner and found by name. A closed, generated
   registry of backend runtime imports remains the structural fix for the first gap.
4. **Lazy compilation.** Not implemented. The runner compiles the whole program before it
   runs it, as cg_clif's current JIT mode does. Measured on the hello program (100
   functions, 62,046 source ops, release runner): TIR and module preparation 556 ms,
   Cranelift IR construction and machine code 292 ms, finalization 1.5 ms. Laziness can
   save at most part of the 292 ms, for functions the run never reaches; module
   initializers, the bulk of a small program, always run. Without JIT redefinition,
   laziness needs Molt-owned call slots, the mechanism "Patched calls" needs too; it is
   built once, there.
5. **CLI.** `molt run --tier jit` (or `[tool.molt.run] tier = "jit"`) selects the JIT
   tier (`RunTier`); `aot` stays the default and `molt build` stays AOT. `molt build
   --emit jit` publishes one JIT image, `<name>_molt.jit`, after the shared frontend and
   backend IR preparation. No tier falls back to another. One admission,
   `run_tier.jit_tier_refusal` (target, host, code generator, installed compiler), runs
   at build request resolution for every `emit=jit` request, so `molt build`, `molt run`
   and the differential harness refuse an unsupported cell before the frontend runs and
   before any runtime image link. External native package artifacts are refused by the
   build's native-artifact custody check, which runs on the import plan before the
   frontend. WASM already runs under wasmtime, which is a JIT.

   A `molt run` cache hit reuses a JIT image only while its key matches. The key adds
   the runner's own source and locked-dependency identity (`_cache_fingerprint` for
   `HostToolProduct.JIT`) to the backend and runtime identity an AOT key holds, so an
   edit to the runner or its `cranelift-jit` stack rebuilds. A hit renews the retention
   of the runner generation and the runtime image it runs.

**JIT image.** A JIT image is one executable file (`src/molt/cli/jit_image.py` writes it,
`runtime/molt-jit/src/image.rs` reads it):

```text
#!/bin/sh
exec <runner> --launch "$0" -- "$0" "$@"
#molt.jit-image.v1 <header bytes> <program bytes>
<header: JSON: runtime image, backend environment, compiler diagnostics, launch plan>
<program: backend IR, MessagePack, the backend lease format>
```

The CLI publishes it with one rename, so a header always belongs to its own program, even
when two builds publish the same name at once. Its first two lines make it a program
`execve` can start: `argv[0]`, and so `sys.executable`, names the image, which starts the
same program again, as an AOT executable's path does. `subprocess.run([sys.executable,
...])` and spawn-mode `multiprocessing` (which executes `sys.argv[0]`) therefore work; each
such start compiles the program again, a cost the AOT tier does not pay. `molt run` and the
differential harness start the runner directly with the same command shape
(`run_tier.jit_runner_argv`), adding `--start-receipt`.

**Launch.** `NativeLaunchPlan` owns a program's launch facts: the resolved capability
environment and `MOLT_EXECUTION_TARGET`, external runtime module roots, and the compiled
source paths. The AOT main stub renders them as C; the JIT image header carries them.
`runtime/native_launch_abi.toml` declares the runtime entry points the launch calls and
their order; `tools/gen_native_launch_abi.py` generates the stub's C declarations and the
runner's `RuntimeLaunchApi` from it. The runner performs the stub's sequence in order:
install the environment, register compiled sources, set the app callable resolver, install
the module registry, `molt_runtime_init`, take the GIL, set argv, call `molt_main`, and
leave through `molt_runtime_exit` after reporting an uncaught exception. The runner's own
failures exit with status 125, as the stub's do. With `--start-receipt <path>` the runner
also creates `<path>` after the program compiled, immediately before the runtime starts,
so a caller can tell a failure to compile from a program that exits with status 125;
`molt run` reports a run with no receipt as a failed build.

## Incremental patching

Zig plans in-place binary patching for sub-millisecond rebuilds, and the Wild linker aims at
incremental linking; neither ships it yet. Molt can get the development-loop effect earlier,
because the JIT tier already owns its program's call graph:

1. **Stable function identity.** The frontend gives each lowered function an exact-byte body
   digest (`molt.cli.function_records.function_body_digest`); unchanged source keeps it.
2. **Reused machine code.** Cranelift's incremental compilation cache
   (`Context::compile_with_cache`, keyed by `compute_cache_key` over the function, with
   `serialize_compiled` and `try_finish_recompile`) returns an unchanged function's code
   without compiling it. Wasmtime exposes it, and Zed cut extension compile time by up to a
   further 41% with it
   ([incremental_cache](https://docs.wasmtime.dev/api/cranelift_codegen/incremental_cache/index.html),
   [Zed](https://git.secluded.site/zed/commit/4ece4a635fb7f525aaafc5fd4f3926f04e4172a8)).
   Cranelift calls it experimental, so it enters behind a differential gate. The AOT backend
   can use the same cache, which shortens every build, not only JIT runs.
3. **Patched calls.** A persistent development session (`molt run --watch`) keeps the runner
   and its compiled code. Calls go through Molt-owned call slots; after an edit the session
   recompiles only functions whose digest changed, points their slots at the new code, and
   runs the program again from its entry. The same slots give lazy compilation (step 4): a
   slot starts at a stub that compiles its function. Program state starts fresh: this is
   fast relaunch, not live update of a running program's objects (`importlib.reload`
   semantics are a separate, later question).
4. **Patched development executables.** No shipped tool patches executables in place yet
   (Zig plans it; Wild aims at incremental linking: [wild](https://github.com/davidlattimore/wild);
   MSVC's `/INCREMENTAL` uses jump thunks). Molt owns its code generator, runtime and link
   plan, so it builds its own: a development executable lays out each function in a slot
   with slack and calls through a thunk table; an edit rewrites the changed slots and
   thunks in place and appends overflow, with the function digests as the identity. Release
   executables keep the ordinary link. This phase follows the JIT tier, which proves the
   function-level identity and the differential gate first.

## Costs

- Removed per edit-run: object emission, the runtime-archive link, output publication,
  output signing, and first-link work for new programs.
- Added once per toolchain: the runner build (the same order as the backend binary).
- Added once per runtime generation: the runtime image link (2.6 s for the 98 MiB
  `stdlib_micro` dev archive on macOS arm64; 37 MiB image). The images of one runtime
  directory form a generation store (`generation_store.py`): an image no build or cached
  launch used for 24 hours leaves at the next image publication.
- Per run: process start, one read of the JIT image, the keeper fork when compiler
  output is captured, runtime image load (3.5 ms), program decode (13 ms for the 1.1 MB
  hello program) and compilation of the whole program, stdlib included (852 ms for
  hello). An AOT build reuses a cached shared-stdlib object; the JIT tier compiles those
  functions on every run, and again in every process that re-runs the image (a spawned
  `multiprocessing` worker, a `subprocess` of `sys.executable`).
- Not removed: frontend lowering on edited modules, backend IR preparation (HF-266) and
  toolchain probes (HF-357). Those lanes run in parallel with this one.

Measured on macOS arm64, 2026-10-21, warm toolchain, host load average 10-14, five runs
each (medians):

| `molt run` of a 13-line program | AOT | JIT |
| --- | --- | --- |
| After a one-line edit, wall | 8.52 s | 6.20 s |
| of it, build phase | 6.03 s | 3.38 s |
| of it, run phase | 0.43 s | 1.17 s |
| New program, wall | 7.95 s | 6.68 s |

The rest of each wall time (2.1 s AOT, 1.7 s JIT) falls outside the timed build and run
phases; this lane did not attribute it. The JIT tier removes the backend
object, link and publication work from the build phase and pays the program's compilation
in the run phase.

The per-run compile does not scale yet. A basic program that imports `traceback` and
`pprint` lowers to 2,119 functions (1.32 M source ops); one JIT run spent 49-76 s
compiling it at load average 20-40, with 2.0-2.5 GB peak RSS. The TIR function cache halved
its IR preparation on the second run (63.7 s to 32.2 s); the module phase and Cranelift
(12-17 s) recompile every run. An AOT build of the same program reuses the cached
shared-stdlib objects. The next step is to give the JIT tier the same reuse: link the AOT
shared-stdlib archive for the program's stdlib set into its runtime image and compile only
the application partition, as an AOT build does, and then add Cranelift's incremental
cache for the edited functions.

## Platform cells

- macOS arm64: verified. `cranelift_jit` 0.137.0 maps anonymous memory and flips it from
  read-write to read-execute with `mprotect` (the `region` crate); it does not use
  `MAP_JIT` or `pthread_jit_write_protect_np`. Cargo-built executables are ad hoc
  linker-signed without the hardened runtime, and the runner works. Under the hardened
  runtime the probe is killed (SIGKILL) unless the executable carries
  `com.apple.security.cs.allow-unsigned-executable-memory`; `allow-jit` alone is not
  enough, because the memory is not `MAP_JIT`. A signed or notarized runner would need a
  `MAP_JIT` `JITMemoryProvider` with write-protect toggling.
- Linux x86_64/aarch64: unverified, so refused. The runtime image would be a shared
  object linked with `--whole-archive`, and the runner exports `molt_isolate_bootstrap`
  with `--export-dynamic-symbol`. `VERIFIED_JIT_HOSTS` (`src/molt/cli/run_tier.py`)
  admits only the cells the twin gate ran; any other host, macOS x86_64 included, fails
  at build request resolution, before the frontend, with a message that names it.
- Windows x64: unsupported. The runner loads its runtime image with `dlopen`, and a DLL
  cannot bind a symbol from the executable without an import library.
- WASM targets: not applicable.

## Verification

- Step 1: identical object bytes before and after the generic-module change (12 of 12
  objects, 7c86a042a) and after the code-owner change (hello object and archive).
- Code owner: `runtime/molt-jit/tests/jit_code_owner.rs` compiles the program that
  `native_direct_symbol_object_call_links_provider_archive_and_executes` links against a
  provider archive, but into memory against the same provider symbols, and runs it; it
  also proves that a missing import or ABI witness refuses the program.
- Runner: `runtime/molt-jit/tests/runner_launch.rs` compiles a recording runtime image
  and a tiny program, runs the real runner, and checks the call sequence against the
  generated launch ABI, that the start receipt exists before `molt_runtime_init`, that the
  runtime binds the runner's `molt_isolate_bootstrap`, and that a program that cannot
  compile leaves no receipt. `compiler_output.rs` tests the keeper: a successful compile
  discards compiler stderr, a failed one replays it first, and a runner killed while it
  compiles still shows it.
- Steps 2-5: the JIT-versus-AOT differential. `tests/molt_diff.py --target native,jit`
  treats the two as exact twins (`EXACT_TWIN_BACKENDS`): they must agree on exit status,
  on stdout under the file's stdout comparison mode, and on stderr exactly after removing
  the text any two runs of one program differ in (heap addresses in default reprs, the OS
  thread id of a Rust panic), whatever the file's CPython law; and a build that succeeds
  for one twin must succeed for the other. The CPython oracle applies as today. The JIT
  tier compiles when it runs, so a run that leaves no start receipt counts as its failed
  build; a timeout or guard failure counts as unknown, not as a divergence. The twins
  share the frontend, TIR, the Cranelift code generator and the runtime bytes, so the
  gate proves that the two code owners and launches run the same program; unlike
  Goose's TinyCC-versus-native diff, it is not an independent implementation and finds
  no shared code-generation defect. Process-state cases pin what a Rust `main` broke:
  `closed_pipe_write_signal.py` (a closed pipe ends both tiers with `SIGPIPE`),
  `native_stack_exhaustion.py` (stack exhaustion ends both with `SIGSEGV`), and
  `multiprocessing_pool_apply_async.py` (spawn-mode workers execute `sys.argv[0]`, the
  image, and so start the program again). The harness sets `sys.executable` to the
  CPython oracle for every compiled program, so `tests/cli/test_jit_run_tier.py` proves
  the `sys.executable` path: executing an image starts its runner with the image as
  argv[0] and the new arguments.
  On macOS arm64 (2026-10-21, main 9e0f94f11, dev compiler, `--jobs 4`, 201 min at load
  10-27), 514 programs ran: the 510 basic programs a native calibration lists as passing,
  without its two expected-failure programs that both tiers pass (XPASS), the two
  process-state cases and two spawn-mode `multiprocessing` programs. 480 pass under both
  tiers; 33 fail CPython identically under both (HF-295 names them and their failure
  classes); one diverges: `multiprocessing_pool_apply_async.py` waits 5 s for a result,
  and its spawned JIT worker is still compiling the program. Two programs that first
  failed on a runtime build lock timeout at cold start (HF-388) passed on a recheck.
  The local proof-queue lane `native.jit-aot-twins` runs the 480 passing cases;
  `native.jit-code-owner` runs the molt-jit tests.
- Latency: edit-run of a small program, new and after a one-line edit, AOT against JIT
  ("Costs"). The target is Goose-class: well under a second for a one-module edit once
  HF-357 and HF-266 land, the untimed CLI overhead shrinks, and the per-run stdlib compile
  goes away.

## Dependencies and decisions

- Molt uses the latest release of every dependency. The native backend, the maintained
  `cranelift-object` patch and wasmtime moved together to Cranelift 0.137.0, wasmtime 50.0.0
  and `object` 0.40.0 (the AArch64 COFF addend patch is still needed: upstream 0.40.0 fails 2
  of its 3 regressions). `cranelift-jit` 0.137.0 joins the same stack with its closure
  (`region` 3.0.2, `mach2` 0.4.3, `bitflags` 1.3.2); the owner approved the download.
- Step 1 (generic module) landed before the upgrade.
