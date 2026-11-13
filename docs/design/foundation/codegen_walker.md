# One codegen walker over final TIR

Status: design, round 1 (2026-11-12). Owner: integrator. Parent: `call_anatomy.md`, "P0 (owner,
2026-11-11): the codegen foundation", item 2. Row: HF-821.

## Problem (measured, origin/main fe2c2896f3, 2026-11-12)

Only LLVM consumes final TIR (`FinalLowering::FinalTir`, `molt-ir` `target_info.rs`). Cranelift,
the default native backend and the JIT's code generator, lowers a SimpleIR copy rebuilt from TIR
(`FinalLowering::BackendLanes`). So every shared fast path landed in the late-TIR stage (C1-C6:
integer `+`/`-`, truthiness, comparisons, inline-int literals, multiply, floats, mixed int/float,
true division) changes LLVM output only; Cranelift keeps its own hand-written copy of each, and
the copies have disagreed (a stale pending-exception byte read as an exception, call_anatomy P0).

| backend | lines (non-test where stated) | input |
|---|---|---|
| Cranelift, `native_backend/` | 57,370 | SimpleIR, name-keyed facts |
| LLVM, `llvm_backend/lowering*` | about 11,600 in op families; 14,752 non-test in the backend | final TIR |
| WASM, `molt-backend-wasm` | about 100,000 | SimpleIR; moving to LLVM wasm32 (worker lane) |

LLVM's lowering is already a walker: `try_lower_tir_to_llvm_with_pgo` declares the function,
creates one LLVM block per TIR block, walks them in reverse post-order (`lower_block`), turns block
arguments into phis, dispatches each op through one exhaustive `match op.opcode` (`op_dispatch.rs`,
119 `OpCode` arms) into op-family methods, and wires phi incomings last (`finalize_phis`). The
families call inkwell directly at about 740 sites (largest: value materialization 145, numeric
100, callable 68, call lowering 53, object 46, frame record 46). Everything that is not one of
those 740 calls is backend-independent walking, dispatch and bookkeeping.

## Decision

Do what `rustc_codegen_ssa` does for LLVM, Cranelift and GCC: one backend-generic walker over
final TIR, generic over a builder trait; each backend implements the trait and nothing else.

1. **The walker** (module `codegen_ssa` in `molt-backend-native`; open question 1): function
   declaration request, block creation, RPO walk, block
   parameters, the exhaustive opcode dispatch and every op family's logic. It owns what LLVM's
   `lowering.rs`, `op_dispatch.rs` and the family modules own today, minus the inkwell calls.
2. **The builder trait** (`CodegenBuilder`, extending the existing `WordBuilder` and
   `FrameBuilder`): typed values (word, i64, f64, bool, pointer), constants, integer and float
   arithmetic and compares, select, casts, loads and stores at generated offsets, blocks,
   block parameters, branches, conditional branches, switch, return, calls by `RuntimeFn` (the
   generated signature, landed 3cbf168b9) and to compiled functions, and the few backend-specific
   attachments (fast-math flags, branch weights, PGO metadata) as optional hints.
   Block parameters are the trait's form, not phis: Cranelift has them natively; LLVM's
   implementation creates the phis and fills incomings at `finish_function`, which is what
   `finalize_phis` does now.
3. **LLVM implements the trait first** by moving its 740 inkwell call sites into the impl; the
   walker logic moves out of `llvm_backend` unchanged. LLVM's output must not change.
4. **Cranelift implements the trait** over `cranelift_frontend::FunctionBuilder`, and switches
   whole: `TargetKind::NativeCranelift` gets `FinalLowering::FinalTir`, and in the same change
   `native_backend`'s SimpleIR op lowering, its name-keyed representation plan and its fast-path
   copies (list index, integer and float lanes, truthiness, guarded calls) are deleted. No
   function is lowered by both paths at any landed commit.

Why whole-backend, not per family: LLVM already lowers every op final TIR can contain (it is a
complete backend, gated by the release matrix), so the walker extracted from it covers the whole
closed op set from its first commit. A per-family Cranelift migration would keep two lowerings of
one function alive across commits, which AGENTS.md forbids ("no internal compatibility lanes").
The cost is one large Cranelift change; its proof is the differential suite, not review alone.

## Migration (each step lands alone, measured)

- **W1. Trait core and walker skeleton; LLVM on it for the walk.** Blocks, block parameters,
  terminators, RPO, dispatch, and the word/frame builders the trait already has. LLVM's walk and
  phi bookkeeping move into the walker. Proof: LLVM release object for each of 10 benchmarks
  byte-identical, or identical instructions-retired and output where LLVM's block naming moves;
  molt_diff basic on `--backend llvm` unchanged; consumer gate.
  What moves and what stays (read of `try_lower_tir_to_llvm_with_pgo`): the walker owns block
  creation in `BlockId` order with the entry first, the RPO walk, block-parameter values, the
  dispatch and the diagnostics list. LLVM's own repairs stay in its `finish_function`: the
  `entry_trampoline` (LLVM forbids predecessors of the entry block; Cranelift does not), the
  `unreachable` fill for unterminated blocks, phi incoming wiring, the `noinline` partition
  attribute and the function-level `unsafe-fp-math` attribute. Two name-keyed reads become typed
  inputs in W1: the per-op `has_attr(op, "fast_math")` string key becomes a typed op fact, and
  `function_repr_facts.get(&func.name)` travels with the `TirFunction` instead of a name lookup.
  First piece (2026-11-12): `codegen_ssa::ValueTable`, one dense slot per `ValueId` holding the
  backend value and its `TirType` together, replaces LLVM's two `HashMap`s (94 paired inserts),
  and refuses a value numbered past `next_value` instead of growing. Obligation found doing it:
  30 LLVM read sites (value materialization, numeric, dispatch, callable, direct, call, state
  machine) treat a value with no recorded type as `DynBox` (`ty(..).unwrap_or(DynBox)`), so a
  lost fact silently selects the boxed runtime path. W1 replaces them with one accessor that
  records a fatal lowering diagnostic for a missing type, as `resolve` already does for a missing
  value, in its own measured commit.
- **W1b. Types from TIR; the table holds representation (measured 2026-11-13).** The LLVM table's
  one `TirType` field carries two facts: the type TIR proved and the representation the backend
  chose (a raw machine word or a boxed word). Experiment: seed the table from
  `TirFunction::value_types` and log each of the 102 per-op `set_type` records (LLVM release, 17
  programs with their stdlib modules, about 306,000 records). TIR types every value (each function
  seeds N of N+1, the remaining slot being function-level). 84% of the records only restate TIR's
  type. The other 16% overwrite a proven type with `DynBox`; attributed by op on
  `bench_math_numeric`: block parameters 11,159 (`I64` phis boxed because one incoming edge does not
  prove the raw carrier: stage 6's loop carriers), `Copy` results about 8,400 (preserved runtime
  ops whose results TIR typed `Tuple` 5,788, `I64` 1,656, `Dict` 546, `List` 151, `Str` 147,
  travelling as boxed words), `GlobalSlotBase` 866 (a raw slot address). Each overwrite drops the
  exact-type fact every later op needs for its fast path, and makes a value that only passed
  through a phi read as unknown.
  Design: the slot holds the backend value and its representation (`Raw(I64)`, `Raw(F64)`,
  `Raw(Bool)`, `Boxed`); the type is read from `TirFunction::value_types`, the one authority, and
  a value TIR left untyped is a fatal lowering diagnostic. The restating `set_type` calls are
  deleted; each remaining site states a representation decision. A boxed word of a proven
  `Tuple` stays a `Tuple`. Proof: the experiment's agreement table re-run shows no type overwrite;
  LLVM release objects of the 10 benchmarks identical, or instructions retired and output
  identical where only representation records move; molt_diff basic on `--backend llvm`; the
  LLVM lowering tests; consumer gate. Stage 6 then unboxes the block-parameter carriers, measured
  on `bench_math_numeric` and fannkuch.
- **W2. LLVM op families onto the trait**, largest call-site count first (value
  materialization, numeric, callable, call lowering, object, frame record, the rest). Each family
  lands when its inkwell calls are inside the impl and its logic is in the walker. Same proof.
  The trait grows only by what a family needs; no speculative methods.
- **W3. Cranelift implements the trait and switches.** Delete in the same change: SimpleIR as
  Cranelift input (`lower_to_simple` stays only while WASM needs it), `native_backend`'s op
  lowering, `repr_by_name`, the Cranelift fast-path copies, `list_index_fast_path.rs`. Proof:
  molt_diff basic and stdlib slices on native with no new failure (each one dispositioned);
  instructions retired and wall time on bench_fib, bench_calls, bench_sum, nbody, spectral_norm,
  fannkuch, mandelbrot, bench_method_defaults against main (no regression over 2%, else fixed in
  the late-TIR stage, once); build latency of a user program, cold and warm, no worse; the JIT's
  tests.
- **After W3**: every new fast path is one late-TIR expansion that both native backends get;
  WASM gets it through LLVM wasm32 (the worker lane). Stages 4, 6 and 7 are written once.

## Contracts and checks

- The walker never names a backend type; the trait never names a TIR op. A method that would
  take an `OpCode` belongs in the walker.
- Runtime calls only by `RuntimeFn` with the generated signature; no string symbol crosses the
  trait (AGENTS.md, "a string match is a compile-time decision made again at run time").
- Facts arrive on typed SSA values (`TirType`, value facts); the walker reads no name-keyed map.
- Exhaustiveness: the dispatch `match` has no `_` arm; an op the target cannot lower is refused
  before codegen by target admission (HF-828), not by a panic in a family.

## Prior art

- rustc: `rustc_codegen_ssa` (`traits::BuilderMethods`, about 200 methods; `mir::codegen_mir` is
  the walker) with `rustc_codegen_llvm`, `rustc_codegen_cranelift` and `rustc_codegen_gcc`.
  Lesson: the trait grew from LLVM's needs; Cranelift's implementation lags where it lacks a
  primitive, so the walker must allow an implementation to expand a method into several
  instructions (Cranelift has no `select` on floats with NaN semantics LLVM has, etc.).
- Zig: one IR (AIR), backends per target, LLVM for release and WASM; self-hosted backends grew
  op by op behind a feature gate per backend, which is the per-family path this design rejects
  because Molt's LLVM backend is already complete.
- Cranelift's own `cranelift_frontend` uses block parameters and SSA variables; the trait's block
  parameters map to it directly.

## Open questions for review

1. Crate placement. Answered by the dependency graph: both native backends already live in one
   crate, `molt-backend-native` (Cranelift behind `native-backend`, LLVM behind `llvm`), and
   `molt-backend` and `molt-jit` reach them only through it. The walker is a module of that crate,
   `codegen_ssa`, compiled under `any(feature = "native-backend", feature = "llvm")`; no new crate.
   `molt-codegen-abi` stays TIR-free (the runtime build uses it) and keeps the word and frame
   generators the trait extends.
2. W1's proof: is byte-identical LLVM IR achievable when value naming moves, or is
   "identical instructions retired and output on 10 benchmarks, plus llvm-diff on the IR" the
   right bar?
3. Cranelift features without an LLVM counterpart today (the JIT tier, `native-object` emission
   options, Cranelift-only `non_escaping_tuple_scalarization`): which are TIR facts the walker
   must honour, and which are backend-private?
4. W3's size: is there a cut of W3 that keeps the no-dual-path rule (for example, the JIT moves
   first, alone, since it is a separate binary), or is W3 one change?
