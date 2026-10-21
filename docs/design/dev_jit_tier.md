# Development JIT tier: run without building

Status: design, 2026-10-20. Ledger: HF-371. Nothing here is implemented.

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
  dynamic libraries, and the driver resolves their symbols through a lookup function. A
  lazy mode compiles each function on its first call
  ([usage.md](https://rust.googlesource.com/rust/+/HEAD/compiler/rustc_codegen_cranelift/docs/usage.md),
  [jit.rs](https://rust.googlesource.com/rust/+/HEAD/compiler/rustc_codegen_cranelift/src/driver/jit.rs)).
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

One code generator, two code owners. The native backend's Cranelift lowering already
builds every function into a `cranelift_module` module; only the module type differs.

1. **Generic module.** The function compiler takes `&mut ObjectModule` in 52 files (164
   mentions). It moves to `&mut dyn cranelift_module::Module`, as cg_clif does; only the
   driver chooses `ObjectModule` (shipping objects) or `JITModule` (memory). This step
   changes no output: object bytes stay identical, which a test proves on the
   differential corpus.
2. **Runner.** A Rust executable, `molt-jit`, links the runtime, the native backend's
   codegen and `cranelift_jit`. It is built once per toolchain and configuration, as the
   backend binary is, and cached by the same identity. It reads the backend IR lease the
   CLI already writes (`BACKEND_IR_LEASE_FORMAT`, MessagePack), compiles, and calls the
   program's entry in-process. Stdout, stderr, exit status and signals are the
   program's own.
3. **Runtime symbols.** Generated code imports runtime functions through
   `Linkage::Import` (34 declaration sites). The JIT resolves them through `JITBuilder::symbol_lookup_fn` over a
   table generated from the runtime-callable ABI manifest (one authority; no hand list).
   A missing symbol fails the JIT before any code runs.
4. **Lazy compilation.** As in cg_clif's lazy mode, each function gets a stub that
   compiles it on first call, so a run compiles only what it reaches.
5. **CLI.** `molt run` uses the JIT tier for development profiles when the runner is
   available for the target; `molt build` stays AOT. The runner is a native-host feature:
   WASM already runs under wasmtime, which is a JIT.

## Costs

- Removed per edit-run: object emission, the runtime-archive link, output publication,
  output signing, and first-link work for new programs.
- Added once per toolchain: the runner build (the same order as the backend binary).
- Per run: process start, runtime initialization and Cranelift compilation of reached
  functions only (lazy). Phase 0 measures today's Cranelift compile time per function on
  the differential corpus, so the per-run cost is a measured number, not an estimate.
- Not removed: frontend lowering on edited modules, backend IR preparation (HF-266) and
  toolchain probes (HF-357). Those lanes run in parallel with this one.

## Platform cells

- macOS arm64: executable memory needs `MAP_JIT` and write-protect toggling, which
  `cranelift_jit` handles; unsigned development executables need no entitlement.
- Linux x86_64/aarch64 and Windows x64: `cranelift_jit` supports them; each is a cell to
  qualify.
- WASM targets: not applicable.

## Verification

- Step 1: identical object bytes before and after the generic-module change, on the
  differential corpus.
- Steps 2-4: every differential program prints the same stdout, stderr and exit status
  under the JIT runner and the AOT build (Goose's TinyCC-versus-native rule), plus the
  CPython oracle as today.
- Latency: edit-run of a hello program and of the 40-module fixture, warm, before and
  after, with process-tree RSS. The target is Goose-class: well under a second for a
  one-module edit once HF-357 and HF-266 land.

## Dependencies and decisions

- `cranelift-jit` at the version that matches `cranelift-codegen` 0.133.1 is not in the
  local Cargo registry; adding it needs one download, which the owner approves.
- Step 1 can start before that download.
