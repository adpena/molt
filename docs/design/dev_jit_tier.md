# Development JIT tier: run without building

Status: steps 1-3 and 5 implemented, 2026-10-21 (HF-371). Step 4 (lazy compilation) and
"Incremental patching" remain. Verified cell: macOS arm64. The tier is slower than AOT
for any program that imports `os` ("Costs"), so it stays opt-in.

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
   requested, as `molt run` shows a backend's stderr. Compiler output must never stop the
   program: the compiler prints a progress line with `eprintln!` every few functions, and
   `eprintln!` panics on a write error. While it compiles, the runner points file
   descriptor 2 at one end of a socket pair. A runner thread drains the other end into
   16 MiB of anonymous shared memory (the ring, which keeps the last 16 MiB and counts the
   rest). The runner holds the reading end until the compile is over, so no compiler write
   can fail (`EPIPE`, `EFBIG`, `ENOSPC`) or raise a signal, and the capture uses no file
   system: not `TMPDIR`, not free disk, not the file size limit. When the compile ends,
   the runner restores fd 2 and discards the text (it compiled) or writes it to the
   restored stderr (it failed). A forked keeper process shares the ring and waits for a
   control pipe to close; the runner never writes to it, so a keeper that another process
   killed changes nothing for the program. If the runner dies while it compiles, the pipe
   closes without the runner's record in the ring, and the keeper writes what nobody has
   shown: the ring, then what is left in the socket. The thread copies each chunk with
   `MSG_PEEK` before it takes it out of the socket and records both steps, so the last
   line before a crash (an allocation failure's message before its abort) is not lost; a
   crash between the record and the take can repeat one chunk. With diagnostics
   requested, the keeper also forwards the ring to the original stderr every 10 ms at
   most, so the process that shows text is the one that records what it showed, and a
   runner crash shows no line twice; if another process kills the keeper, the runner
   shows the rest when the compile ends. The forwarding keeper reads the ring while the
   drain thread writes it, without a lock. A keeper a whole ring (16 MiB) behind can
   therefore print a stretch the thread is overwriting, as mixed old and new bytes; those
   are bytes the ring was about to drop. Only
   diagnostics mode forwards, so the default mode never shows such text. Each record
   follows its write, so a killed
   process can repeat a chunk but never lose one. The keeper is reaped before the program
   starts. The capture's unit tests run each scenario in a fresh copy of the test binary,
   not in a fork of the multi-threaded test harness, where a lock another test thread
   held at the fork (Rust's thread-start lock) hung the capture's thread.

   The capture never changes how the program starts. A closed fd 2 stays closed for the
   program, but the capture occupies it while the runner compiles, so no file the compiler
   opens can become fd 2 and receive compiler output. Every descriptor the capture opens
   sits above fd 2 and closes on exec, so a closed fd 0 or 1 stays closed. If the capture
   cannot be set up (no descriptors, memory or thread), the runner says so on stderr and
   the compiler writes to fd 2 directly, as with diagnostics: the program still starts.
   Only then can an error on fd 2 reach the compiler. The runner's own lines ignore write
   errors. While it works, the runner ignores `SIGPIPE` and `SIGXFSZ`, as CPython does for
   its whole life, so a broken pipe or the file size limit fails only that write (a
   forwarded diagnostic, a TIR cache file, the start receipt); it restores the
   dispositions it inherited before the program starts, and leaves a pending signal
   pending. The persistent TIR cache lives in the runner's namespace under the cache root
   the CLI resolved and wrote into the image (`tir_cache_root`), never under the program's
   `MOLT_CACHE` or current directory; the image parser refuses a relative root, which would
   resolve against the program's current directory; a cache write that fails only loses that entry.
   `runner_launch.rs` starts images with `TMPDIR` unusable and `MOLT_CACHE` unset (the
   current directory stays empty), with a file size limit of zero and `SIGXFSZ` at its
   default, with one free descriptor (the runner streams), with stderr a broken pipe, and
   with only the keeper killed while the compiler prints progress for 20,000 functions;
   each program starts and exits 0, and the failing compile still exits 125. The twin
   case `start_ignores_caller_file_state.py` starts the same program under an unusable
   `TMPDIR` and under `ulimit -f 0`, and checks its directory stays empty, on both tiers
   and CPython. The v4 capture file failed the first two (exit 125, or the compiler's
   panic), and left `.molt_cache` in the program's directory.

   macOS's allocator keeps freed large blocks in a cache, so the compiler's freed heap
   would stay in the program's footprint (522-570 MB for a program that imports
   `subprocess`, against 6.7 MB under AOT). `malloc_zone_pressure_relief` releases none
   of it, and libmalloc reads its settings only at process start. The runner therefore
   restarts itself once with `execv` (same pid, descriptors, signal mask and
   dispositions) with `MallocLargeCache=0` and its own first argument
   `--allocator-restarted`, and drops both before anything else runs. The marker travels
   in argv, so no variable a caller passes can stand for it; a caller that sets
   `MallocLargeCache` itself keeps its own setting and gets no restart. On glibc the
   runner calls `malloc_trim(0)` before the program starts. The image bytes are dropped
   first. The setting holds for the whole program, a named trade ("Costs").
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
4. **Lazy compilation.** Not implemented. It is the structural fix for the per-start
   cost ("Costs"). The runner compiles the whole program, its stdlib closure included,
   before it runs it, as cg_clif's current JIT mode does. `import os, sys` lowers to 832
   functions (517,441 source ops); each start spends 4.1-5.2 s in IR passes and 2.7-2.8 s
   in Cranelift (macOS arm64, load 10-12), mostly on functions the run never calls.
   Leading JITs compile a function only when it is first called: HotSpot compiles no
   method before it runs, V8 compiles JavaScript functions lazily and WebAssembly
   functions with lazy Liftoff compilation on first call, and cg_clif's former lazy JIT
   mode gave each function a Cranelift stub that compiled it on its first call. They
   then reuse compiled code across starts through a content-keyed cache: V8's code cache
   and Wasmtime's compilation cache key machine code by the bytes it was compiled from.
   Molt needs both: call slots that Molt owns and a first call fills (`cranelift-jit`
   0.137.0 has no redefinition, and "Patched calls" needs the same slots), and a cache of
   each function's machine code keyed by its IR and the code generator's identity.
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

   A `molt run` cache hit reuses a JIT image only while its key matches. A JIT launch
   runs the runner and never the backend compiler, so its key holds the runner's source
   and locked-dependency identity (`_cache_fingerprint` for `HostToolProduct.JIT`) with
   the runtime's, in place of the backend's an AOT key holds: an edit to the runner, its
   `cranelift-jit` stack, a crate it builds on or a file such a crate's build reads
   (`package.metadata.molt.build-inputs`, below) rebuilds. One fingerprint per key costs
   271-320 ms warm (median, load 11), where the AOT key's costs 433-464 ms; the earlier
   key, which added the runner's fingerprint to the backend's, cost 661-867 ms. A hit
   checks and renews, under each store's lock, the runner generation and the runtime
   image it runs; an entry that eviction removed is a miss.

   **Build inputs outside a crate.** A crate declares the files outside its root that its
   build script, `include_str!` or `#[path]` modules read in `[package.metadata.molt]
   build-inputs`, relative to the crate as `cargo:rerun-if-changed` paths are
   (`runtime/molt-ir/Cargo.toml` declares `src/molt/backend_environment.json`). Every
   source closure that reaches the crate keys them: the runner, the backend, the native
   runtime and the CPython ABI. After each build, Cargo's own reads (rustc dep-info and
   build-script `rerun-if-changed` lines, `cargo_build_reads`) must lie inside the
   closure, or the backend, runner or runtime generation is not published
   (`uncovered_build_inputs`), as Bazel refuses an undeclared inclusion
   (`docs/design/cargo_input_authority.md`).

**JIT image.** A JIT image is one executable file (`src/molt/cli/jit_image.py` writes it,
`runtime/molt-jit/src/image.rs` reads it):

```text
#!<runner> --image
#molt.jit-image.v3 <header bytes> <program bytes>
<header: JSON: runtime image, backend environment, compiler diagnostics, TIR cache root,
         launch plan>
<program: backend IR, MessagePack, the backend lease format>
```

The CLI publishes it with one rename, so a header always belongs to its own program, even
when two builds publish the same name at once. It is created executable through the umask,
as a linker creates an AOT executable (0o777 less the umask). Its first line makes the
runner its interpreter: `execve` of the image starts `<runner> --image <image> [args...]`
with no shell between, so the program keeps the environment its caller passed, and
`argv[0]`, and so `sys.executable`, names the image, which starts the same program again,
as an AOT executable's path does. `subprocess.run([sys.executable, ...])` and spawn-mode
`multiprocessing` (which executes `sys.argv[0]`) therefore work; each such start compiles
the program again, a cost the AOT tier does not pay. `molt run` and the differential
harness start the runner with the same command (`run_tier.jit_runner_argv`), adding
`--start-receipt`. The runner's flags (`--probe` among them), its start receipt and the
image marker come from one declaration, the `[runner]` table of `runtime/native_launch_abi.toml`, which
`tools/gen_native_launch_abi.py` projects into Python and Rust as it does the launch
entries. The kernel ends an interpreter path at whitespace (and macOS's parser also at
`#`) and reads only the line's first bytes (256 on Linux, 512 on macOS), so publication
refuses a runner path that is relative, holds whitespace, `#` or a NUL, or makes the line
too long.

Two differences from a directly started AOT executable remain, both from how the kernel
starts an interpreter: `argv[0]` is the path the kernel executed (absolute when a shell
found the image through `PATH`, where an AOT executable keeps the name its caller gave),
and `ps -o comm` names the runner, not the image. The environment, descriptors and signal
state are the caller's: `runtime/molt-jit/tests/runner_launch.rs` executes an image from a
caller with a stale `PWD`, an `OLDPWD` and no `SHLVL`, and the twin case
`reexec_keeps_parent_environment.py` restarts each tier by its own path with the same
environment and checks that both see it unchanged.

An image is a development artifact. Its header names the runner, the runtime image and
the runtime callable-symbol table, three entries of the host's product store (design
56, section 2.8), and running the image renews none of them. `molt run --tier jit` and
`molt build --emit jit` renew all three (a cache hit too); after 5 days with neither, a
later build may evict them, and `molt cache evict` or removing the shared Cargo target
directory removes them at once. The image then fails: the kernel cannot start its interpreter, so a shell reports a bad interpreter
(exit 126) and `execve` fails with `ENOENT` (Python raises `FileNotFoundError` naming the
image), or the runner cannot load the runtime image (exit 125).
`tests/cli/test_jit_run_tier.py` proves the first case.

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

The JIT tier is slower than the AOT tier for any program that imports `os`, and it needs
about 0.7 GB at its peak to compile such a program. It stays opt-in (`--tier jit`) until it
reuses compiled stdlib code, as an AOT build does.

- Removed per edit-run: object emission, the runtime-archive link, output publication,
  output signing, and first-link work for new programs.
- Added once per toolchain: the runner build (the same order as the backend binary).
- Added once per runtime generation: the runtime image link (2.6 s for the 98 MiB
  `stdlib_micro` dev archive on macOS arm64; 37 MiB image). Each image is a product store
  entry of kind `jit-runtime-image`, keyed by the archive digest and the link command, and
  never a write into the runtime's own entry: an image no build or cached launch used for
  5 days leaves at the next publication. The store's 5-day window keeps up to 5 days of
  images where the earlier per-runtime store kept 24 hours: 37 MiB per runtime generation
  run under the JIT tier in that window, for one retention policy across the store.
- Per start: process start, one read of the JIT image, the compiler output capture (a
  socket pair, 16 MiB of shared memory touched only as far as the compiler writes, one
  drain thread and the keeper fork), the runtime image load, the program decode, and
  compilation of the whole program, its stdlib closure included. An AOT build reuses a cached shared-stdlib
  object; the JIT tier compiles those functions on every start, and again in every
  process that starts the image again (a spawned `multiprocessing` worker, a
  `subprocess` of `sys.executable`). On macOS the runner restarts itself once to turn off
  the allocator's large-block cache, so the compiler's freed heap leaves the program's
  footprint. That is a named trade: about 0.3 s of compile time per start, and libc's
  large blocks lose their cache for the whole program (below).
- Not removed: frontend lowering on edited modules, backend IR preparation (HF-266) and
  toolchain probes (HF-357).
- Kept: the persistent TIR cache, written at start under the image's cache root. For a
  program that imports `subprocess` (v4 runner, 2026-10-22, three starts each at load 9),
  a warm start took 12.1-12.2 s, a start with no usable cache 14.3-14.5 s, and a cold
  start, which writes 49 MB of cache, 15.2-15.3 s. The cache saves 2.2 s on every start
  after the first and costs 0.9 s once.

Measured on macOS arm64, 2026-10-21, release runner, warm toolchain. Per start of one
image, by cost bucket (`MOLT_BACKEND_TIMING=1`, `/usr/bin/time -l`; three starts at load
10-12, and the v3 review's starts at load 8-16):

| Bucket, per start | `print(42)` (100 functions, 62,046 ops) | `import os, sys` (832 functions, 517,441 ops) |
| --- | --- | --- |
| Runtime image load | 3.5 ms | 3-154 ms (154 ms cold) |
| Program decode | 13 ms | 85-118 ms |
| IR passes (TIR, module preparation) | 556 ms | 4.11-5.24 s |
| Cranelift codegen | 292 ms | 2.71-2.78 s |
| JIT finalize (link) | 1.5 ms | 13 ms |
| Compile, total | 0.73-1.13 s | 6.84-8.02 s (9.42-10.17 s at load 8-16) |
| Start to exit, wall | 0.86-1.92 s | 7.36-9.34 s (10.0-10.7 s at load 8-16) |
| Peak RSS | 168 MB | 0.94-0.97 GB (0.97-1.01 GB); 694-767 MiB with the allocator restart |
| The AOT executable, wall and peak RSS | | 0.27-0.35 s, 23 MB |

While a program that imports `subprocess` runs (2026-10-22, release runner, five
interleaved starts each at load 8-12, `vmmap --summary`): its physical footprint is
72.9-74.8 MB under JIT with the allocator restart, 875-895 MB without it, and 6.7 MB
under AOT. With the v4 runner, which also captures compiler output in a file, the
footprint is 72-74 MB with the restart and 522-570 MB when the caller keeps the cache
(`MallocLargeCache=1`), at load 60-180.

The large-block cache is off for the whole program, not only for the compiler. Molt's
own heap does not use libc `malloc` (the runtime image allocates through mimalloc):
churning `bytearray`s of 128 KiB, 1 MiB and 8 MiB costs the same under JIT with the
restart, under JIT with the cache kept, and under AOT (2.9-3.4, 7.5-11.2 and 38-49 us
each, load 117-180). libc `malloc` does use it: C extensions and `PyMem_Malloc` blocks.
A C loop that mallocs, touches and frees one block costs 5.9-7.3 us against 0.1 us at
128 KiB, 45-55 us against 0.3-0.5 us at 1 MiB, and 427-498 us against 2.3-2.5 us at
8 MiB with the cache off. The JIT tier admits no external native package artifacts
today, so no admitted program pays this; a program that loads C extensions under the
JIT tier must set `MallocLargeCache=1` to keep the cache, or the restart must give way to
a compiler heap of its own. The restart cost 0.28 s of compile time for `import os, sys` (median 7.33 s
against 7.05 s), lowered its peak RSS from 796-849 MiB to 694-767 MiB, and added nothing
measurable to `print(42)` (0.90-1.06 s against 0.86-1.08 s). `ps` still reports about
1.0 GB of RSS under JIT (28 MB under AOT): macOS counts freed pages the allocator marked
reusable as resident until it needs them, and the physical footprint is the figure macOS
charges the process. Through `molt run` (v3 review, `import os, sys`, load
9-16):

| `molt run` | AOT | JIT |
| --- | --- | --- |
| New program, wall | 28.45 s | 17.01 s |
| No-change re-run (cache hit), wall | 1.59-1.61 s | 9.23-10.06 s |
| of it, run phase | 0.34-0.36 s | 7.80-8.49 s |
| After a one-line edit, wall | 7.90-11.45 s | 14.76-38.00 s |
| of it, build phase (frontend, IR, output) | 5.60-8.67 s | 2.79-3.81 s |

The JIT build phase after an edit (frontend lowering, backend IR preparation and the
image write) is 2.8-3.8 s, against 5.6-8.7 s for an AOT build; this lane has not yet
split it by phase. The launch key adds 271-320 ms warm ("CLI"). A program without stdlib
imports is the one case the JIT tier wins: a 13-line program ran in 6.20 s after a
one-line edit against 8.52 s under AOT (medians of five runs, load 10-14).

The per-start compile does not scale. A basic program that imports `traceback` and
`pprint` lowers to 2,119 functions (1.32 M source ops); one JIT start spent 49-76 s
compiling it at load average 20-40, with 2.0-2.5 GB peak RSS. The TIR function cache
halved its IR preparation on the second start (63.7 s to 32.2 s); the module phase and
Cranelift (12-17 s) recompile every start.

The structural path (HF-371) is to compile each function lazily on its first call and to
reuse machine code through a content-keyed cache ("Lazy compilation"). A shorter step uses
what the AOT tier already has: link the AOT shared-stdlib archive for the program's
stdlib set into the runtime image and compile only the application partition.

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
  discards compiler stderr, a failed one replays it first, a runner killed while it
  compiles still shows it, and a keeper killed alone changes nothing.
- Steps 2-5: the JIT-versus-AOT differential. `tests/molt_diff.py --target native,jit`
  treats the two as exact twins (`EXACT_TWIN_BACKENDS`): they must agree on exit status,
  on stdout under the file's stdout comparison mode, and on stderr exactly after removing
  the text any two runs of one program differ in (heap addresses in default reprs, the OS
  thread id of a Rust panic), whatever the file's CPython law; and a build that succeeds
  for one twin must succeed for the other. A divergence between any two backends fails
  the file whatever its `expect_fail` says. An expected failure passes only when every
  backend ends exactly the way the file pins (`expect_molt_returncode` and
  `expect_molt_stdout_sha256`); any other ending fails it, and exact twins require the
  pin, so two tiers that fail the same new way never read as the known gap. The CPython oracle applies as today. The JIT
  tier compiles when it runs, so a run that leaves no start receipt counts as its failed
  build; a timeout or guard failure counts as unknown, not as a divergence. The twins
  share the frontend, TIR, the Cranelift code generator and the runtime bytes, so the
  gate proves that the two code owners and launches run the same program; unlike
  Goose's TinyCC-versus-native diff, it is not an independent implementation and finds
  no shared code-generation defect. Process-state cases pin what a Rust `main` broke:
  `closed_pipe_write_signal.py` (a closed pipe ends both tiers with `SIGPIPE`),
  `native_stack_exhaustion.py` (stack exhaustion ends both with `SIGSEGV`),
  `multiprocessing_pool_imap.py` (spawn-mode workers execute `sys.argv[0]`, the image,
  and so start the program again), `reexec_keeps_parent_environment.py` (a restart by the
  program's own path keeps the parent's environment and arguments) and
  `closed_stderr_start.py` (a program started with fd 2 closed runs under both tiers; both
  then see the runtime's `/dev/null` on fd 2 where CPython sees it closed, an expected
  failure until the runtime keeps a closed fd 2 closed, HF-455). The harness sets `sys.executable` to the
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
  The lane now runs those 480 and two more process-state cases, `closed_stderr_start.py`
  and `reexec_keeps_parent_environment.py`. On the lane tip (2026-10-22, main 1c317eeb8,
  126 min at load 10-60) all 482 pass under both tiers, and the three pinned expected
  failures end exactly as pinned; 8 native builds first failed on 120 s backend-daemon
  lock timeouts (HF-388) and passed on a recheck. Each full run so far lost cases to an
  HF-388 lock timeout (2, 1 and 8 in the last three runs), so a full run is usually red
  until HF-388 lands, and its timeouts need a recheck. The proof
  queue cannot run the lane on macOS, its only admitted host: the macOS proof supervisor
  refuses every process-closure mode (HF-07), so the queue refused `native.jit-aot-twins`
  with "native supervisor prelaunch refused" (2026-10-22). The lane argv runs detached
  with `REAPER_KEEPALIVE=1` and a kept log until HF-07 admits a macOS closure.
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
