# 76. One SimpleIR optimization pipeline for every target

Status: implemented 2026-10-11 (HF-87 follow-up). Owner: compiler midend. Each pass in the table
retires when TIR covers its fact family and backends consume typed IR
([77_architecture_convergence.md](77_architecture_convergence.md)).

## Problem

Before the TIR lift, each backend ran SimpleIR passes it listed by hand.
`molt-backend-native` (`program_pipeline.rs`) and `molt-backend-wasm`
(`compile_pipeline.rs`) kept two copies of one sequence, and the copies drifted
during crate extraction:

| Step | Native | WASM | Luau | Rust |
|---|---|---|---|---|
| `rewrite_stateful_loops` .. `elide_safe_exception_checks` | yes, parallel | yes, serial | no | no |
| `escape_analysis` (marks only Cranelift consumes) | yes | yes, unused | no | no |
| `fold_constants_cross_block` | yes | missing | no | no |
| `elide_safe_exception_checks` | yes | missing | no | no |
| extern functions skipped | no | yes | n/a | n/a |

Luau and Rust ran no SimpleIR optimization at all. The passes therefore looked
dead in builds without the native and WASM features, and carried
`cfg_attr(..., allow(dead_code))` masks.

## Design

The shape follows established pass managers:

- **One declarative, ordered pass table**, as GCC's `passes.def`: the order is
  data in one place, not code repeated per backend.
- **Typed target queries, not backend-chosen pass lists**, as LLVM's
  `TargetTransformInfo`: a pass states the capability it needs, and the
  target description answers. IR transforms never depend on a backend crate.
- **Per-function parallel execution**, as MLIR runs passes over
  isolated-from-above operations: each SimpleIR pass reads and writes only its
  own `FunctionIR` and keeps no state between functions, so functions run
  concurrently and the result does not depend on scheduling.

### Authority

`molt_tir::passes::pipeline` owns the order of the function stage, run in
parallel over every non-extern function: `rewrite_stateful_loops`,
`eliminate_unbound_local_checks`, `eliminate_redundant_guard_tags`,
`elide_dead_struct_allocs`, `escape_analysis`, `rc_coalescing`,
`fold_constants`, `fold_constants_cross_block`,
`elide_safe_exception_checks`. HF-216 deleted the tenth pass,
`fuse_method_dispatch`: the frontend emits CPython's two-value method load
(`load_method`, `load_super_method`, `call_loaded_method`) directly.

Every backend calls `run_simple_ir_pipeline(&mut ir, &target_info)` with its
`TargetInfo`; the native backend passes its lane's target, Cranelift or LLVM.
Function layout stays with the code-generating backends: native and WASM call
`apply_profile_order` themselves, and megafunction splitting stays
backend-owned (native partitions for task custody, WASM for function-size
limits), after the pipeline.

### Gates

A pass declares the op kinds it can introduce and, if any, the backend
lowering feature it relies on. It runs on a target when both hold:

1. the target supports the generated runtime requirements
   (`op_kinds.toml`, `simpleir_runtime_requirements_table`) of every
   introduced kind, the same authority target admission reads; and
2. the target's `TargetInfo::lowering` (`SimpleIrLoweringFeatures`, one
   exhaustive answer per `TargetKind`) implements the feature.

| Pass | Gate | Native | WASM | LLVM | Luau, Rust, MLIR |
|---|---|---|---|---|---|
| `rewrite_stateful_loops` | `state_*` kinds need `ASYNC_RUNTIME` | yes | yes | yes | no |
| `escape_analysis` | non-escaping tuple scalarization | yes | no | no | no |
| the seven others | none: they remove ops or fold to `const`/`const_bool` | yes | yes | yes | yes |

RC coalescing needs no gate: it removes matched `inc_ref`/`dec_ref` pairs, and
targets without reference counting already treat those ops as no-ops.

A gate is a static capability, not a trial: no pass runs and is then checked
for admission, because a failed trial is not feature detection.

## Consequences

- WASM gains cross-block constant folding and safe-exception elision and stops
  running escape analysis it never consumed; the LLVM lane likewise; Luau and
  Rust gain every target-neutral pass. The native, WASM, Luau and Rust
  differential suites prove each target.
- `TargetInfo::lowering` joins the TIR cache fingerprint.
- The per-backend sequences and the passes' dead-code masks are deleted.
- A new SimpleIR pass is added once, to the table, with its requirement.

## Later work

Cranelift's ægraph replaces separately ordered machine-independent passes with
one rewrite engine and removes the phase-ordering problem. Molt's TIR pipeline
is the place to evaluate that; this table only fixes duplication and drift.
