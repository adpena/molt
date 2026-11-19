# One codegen walker over final TIR

Status: W1, W1b and W2 slices 1-6 built; Cranelift implements the builder (W3 step 1); W2 families and the W3 switch continue (2026-11-18). Owner: integrator. Parent: `call_anatomy.md`, "P0 (owner,
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
| WASM, hand-written code generator | about 100,000 when deleted | replaced by LLVM wasm32 (`e605908087`) |

LLVM's lowering is already a walker: `try_lower_tir_to_llvm` declares the function,
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
  What moves and what stays (read of `try_lower_tir_to_llvm`): the walker owns block
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
  Design: the slot holds the backend value and its held carrier (`HeldCarrier`: `RawI64`,
  `RawF64`, `RawBool`, `Never`, `Boxed`); the type is read from `TirFunction::value_types`, the
  one authority. The held carrier is not the planned carrier: `molt_ir::repr::Repr` (the
  representation plan, `repr_facts`) is the proof-level lattice of what a value may be held as,
  and the held carrier is the instruction-selection fact of what the backend holds. Measured
  against the plan (bench_math_numeric, bench_calls, fannkuch): 98.6% agree; the backend holds
  1,782 planned-boxed ints and 1,188 planned-boxed comparison results raw and 268 planned-raw
  values boxed. Each deviation converges on the plan or moves into it; then the held carrier is
  the plan's carrier and the table stops recording it. The restating `set_type` calls are
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
  As built (2026-11-17, lane codegen-walker):
  - The trait is `codegen_ssa::CodegenBuilder`. It extends `molt-codegen-abi`'s `BranchBuilder`, so
    one backend value spells the shared generators and the walker. LLVM implements it in
    `llvm_backend/lowering/codegen_builder.rs` (`LlvmCodegen`).
  - Order: bottom-up, callee before caller. A walker function cannot call a family that is still
    LLVM-only, so the leaves move first and the dispatch moves last. Until a family's callers move,
    the LLVM method that remains binds the builder and calls the walker.
  - Moved, slice 1: scalar carriers (`codegen_ssa::scalar`): a value as a word, box, unbox, the
    direct-call parameter decode, truth. LLVM's six builder-parameterized twins of them are deleted.
  - Moved, slice 2: raw numeric lanes (`codegen_ssa::numeric`): `+ - *`, comparisons, bitwise and
    shifts, unary ops, each with its result-range admission. A lane answers `None` when no raw lane
    is proven; the boxed runtime call stays in LLVM until operand custody moves.
  - Proof of both: the `__text` section of 37 LLVM release binaries (10 benchmarks, 27 idioms) is
    byte-identical to main's; model tests run each walker function on an interpreting builder
    against Python's semantics.
  - Moved, slice 3: checked add and multiply (the overflow peel's ops) and zero-guarded division
    (`numeric::checked_arith`, `numeric::divide`): Python's `//` floor and `%` sign rules, the float
    `%` sign and signed-zero rules, and the zero arm that raises through the boxed entry, written
    once. The zero arm boxes only the zero divisor, so it mints no owner and needs no operand
    custody. Fixed on the way: LLVM ran a raw `sdiv`/`srem` on a full-range dividend, which is
    undefined for `i64::MIN // -1` and `i64::MIN % -1` (Python: `2**63` and `0`). Now `//` needs
    an inline-safe dividend or a proven inline-safe quotient (the contract Cranelift already
    keeps), `%` of a full-range dividend divides by 1 where the divisor is -1, and `/` needs both
    operands inline-safe (an int converts to a float exactly only below 2**53).
  - Remaining trait surface, by LLVM call-site count (about 650 sites): runtime and compiled calls
    101, branches and blocks 62, loads and stores 60, integer compares 32, address casts 44,
    returns 18, block parameters (phis) 14, address arithmetic 15, entry stack slots 12, select 5,
    switch 2, atomic update 2. Remaining families in order: operand custody and runtime calls
    (value materialization), terminators and block parameters, callable, call lowering, object, frame record, containers, constants,
    state machine, task construction, then the walk and dispatch.
  - Moved, slice 4 (2026-11-18, round 3): block parameters and branch terminators
    (`codegen_ssa::control`). The trait gained `Block`, `append_block_param`, `jump`,
    `branch_if`, `switch`, `unreachable` and `placeholder`. A branch converts each argument to
    its parameter's carrier where the branch is built, before it (`control::coerce`: one
    conversion for edges, returns, entry parameters and a direct call's raw parameters). This
    closes the W3 obligation that `coerce_to_tir_type` inserted conversions before an existing
    terminator, which Cranelift's `FunctionBuilder` cannot. LLVM builds each parameter as a phi
    and adds incomings as edges are built. Deleted: `finalize_phis`, the pending phi and edge
    lists, `coerce_to_tir_type`, `coerce_to_type`, `patch_incomplete_phis` (an incomplete copy
    of LLVM's own verifier check: blocks made by `choose` were never recorded), the
    predecessor map and block list with their 57 recording sites, and the PGO branch-weight
    parameter, which never attached a weight. `Return` and the generator state dispatch read
    frame state and stay in LLVM until the frame-record and state-machine families move; both
    call the walker's coercion and switch. Lines: LLVM lowering -862 +402; walker +306 non-test
    (`control.rs` 274, trait 32) and +332 test; Cranelift 0.
  - Moved, slice 5 (2026-11-18, round 4): operand custody and runtime calls
    (`codegen_ssa::custody`). One custody per operation boxes each borrowed operand once,
    gives only a raw integer without an inline-safe proof an entry owner slot, joins every
    failure in one block, releases a call's adopted inputs on that path only, and releases
    each minted owner once after the consumer; a runtime call borrows through it. The trait
    gained `create_block`, `switch_to_block`, entry slots (`entry_slot`, `entry_slot_array`,
    loads, stores, address), `refcount_site`/`refcount` (the transition stays the shared
    `inline_refcount` generator; a backend chooses only inline or out-of-line placement),
    `runtime_callee`/`call` (a callee is a runtime entry or a backend helper), and debug
    names (`create_block` takes one, `set_name` names a value; rustc's `append_block` and
    `set_var_name` precedent). `call_runtime` is now a provided method over `call`. LLVM's
    per-function refcount state moved into `LlvmFunctionState`, which `LlvmCodegen` carries;
    the refcount helper builders moved to `LlvmBackend`. LLVM keeps thin wrappers that bind
    the builder until the custody callers (calls, containers, objects) move with their
    families. The exception-pending read and result binding stay in LLVM until the frame and
    dispatch families move.
  - Moved, slice 6 (2026-11-18, round 4): the walker context (`codegen_ssa::function::FunctionCx`)
    owns the mutable value table while an op lowers, beside the builder; a backend lends both
    as disjoint borrows of its function state (LLVM: `FunctionLowering::walker`). Result
    binding is written there once: runtime-call result custody (`ResultCustody`, moved from
    LLVM), owned-result binding, `BoxVal`, `UnboxVal`, `IncRef` and `DecRef`. Proof: LLVM
    `__text` byte-identical on the 38 binaries.
  - Moved, slice 7 (2026-11-18, round 5): constants (`codegen_ssa::constants`): the scalar
    literals, `ConstNone`, the str, bytes and big-integer literals (a private byte constant
    the runtime materializes into one owned object per evaluation) and `ModuleSlot`; the byte
    literal is shared with LLVM's `with_owned_name`, which moves with its consumers (object
    family). The trait gained `bit_constant`, `bytes_constant`, `program_slot` and
    `slot_pointer` (which replaces `slot_address`: one fact, the slot's address, and a word
    when a caller needs one), and `call` now returns a general value, so a 32-bit runtime
    status keeps its kind on every builder. Cranelift's program-slot load moved beside its
    runtime imports for both its paths. The model tests read pointer arguments through one
    memory model on both machines.
  - Moved, slice 8 (2026-11-18, round 5): objects (`codegen_ssa::object`): `LoadAttr`,
    `StoreAttr`, `DelAttr` (named, dynamic-name, inline-cache, guarded-field and proven-field
    forms), `Index`, `StoreIndex`, `DelIndex`, and the synthesized-name owner
    (`begin_owned_name`/`finish_owned_name`, which LLVM's call family still uses). A proven
    field or list item access is a `SiteBody` the backend places through `site`: LLVM keeps one
    always-inlined helper per body (its blocks may not split the caller's block), Cranelift
    inlines the same `molt_codegen_abi` generator in the caller. A borrowed call enters a
    `CallTarget` (callee or site body). The trait also gained `refcount_admission` and
    `is_zero` (a zero test in the value's own width, so LLVM's IR keeps `icmp eq i32`). LLVM's
    `object_ops.rs` is deleted; its field and list helpers and `site_helper` moved to
    `LlvmBackend`. The model tests run the site bodies' non-object arms as Cranelift machine
    code and the named accesses' failure path.
  - Moved, slice 9 (round 5): operators (`codegen_ssa::operators`): arithmetic with the checked
    ops and in-place entries, comparison, bitwise, unary, identity, membership, `and`/`or`
    selection, `Bool` and the late stage's word primitives. LLVM's `numeric_ops.rs` lowering
    is deleted; its preserved numeric arms call the walker.
  - Moved, slice 10 (round 5): runtime calls (`codegen_ssa::calls`): classified boxed and raw
    runtime calls with their availability admission, preserved-op runtime routing, CallArgs
    and the dynamic call protocol. `FunctionCx` carries the selected runtime's entry set and
    the inline-cache call-site counter.
  - Moved, slice 11 (round 5): containers (`codegen_ssa::containers`): list, tuple, dict and
    slice literals, the guarded class definition, and the iterator entries that write through
    an entry array; custody's eager form (`with_words`). The set and frozenset aggregate stays
    in LLVM until the pending-exception read moves (it checks for a pending exception after
    each insertion).
  - Moved, slice 12 (round 5): module globals (`codegen_ssa::globals`): static cells, global
    slots and closed imports; the slot read, cell store and closed import are site bodies.
  - Moved, slice 13 (round 6): the call family's runtime routes (`codegen_ssa::calls`): `Call`
    (source, builder-consuming, guarded and runtime-role calls, the dynamic call, the refusal
    of an unlinked direct target), `CallMethod`, `LoadMethod`/`LoadSuperMethod`,
    `CallargsPushMethodReceiver` and `CallBuiltin` (print, range, named and dynamic builtins).
    A method site's load is a site body (`SiteBody::MethodSiteLoad`) against a record the
    builder creates once (`site_record`: LLVM an internal writable global, Cranelift local
    writable data). LLVM's `with_owned_name`, `with_callargs` and dynamic-call wrappers are
    deleted. The direct call of linked compiled code, `CodeIs`, `DirectCall` and the
    loaded-method call stay in LLVM: the next slice moves the first two; the other two pass
    the thread record, which the walker builder cannot read yet.
  - Moved, slice 14 (round 6): the direct call of linked compiled code and `CodeIs`
    (`FunctionCx::compiled_call`, `code_is_op`). `FunctionCx` carries two more program facts,
    each compiled function's linkage row and the function table, so the walker resolves a
    call's target itself. The trait gained `function_callee` (declared from the linkage row:
    LLVM its module declaration, Cranelift an import of the attached execution symbol with one
    parameter per row carrier) and `function_address`. `DirectCall` keeps only its
    thread-record handoff in LLVM around the walker's call.
  - Moved, slice 15 (round 6): the direct ops (`codegen_ssa::direct`): `DeleteVar`, the `Copy`
    no preserved-op lowering claimed (an SSA copy passes operand 0 through in its carrier; a
    preserved origin is refused, as `alias_analysis::CopyLowering` requires), `Alloc`, `OrdAt`,
    `Raise`, `WarnStderr`, `FunctionDefaultsVersion`. `StackAlloc` (refused) and `Free` (no
    code) move with the dispatch.
  - Moved, slice 16 (round 6): the callable family's runtime calls (`codegen_ssa::callable`):
    `function_closure_bits`, `asyncgen_new`, `code_slot_set`, `code_slots_init`,
    `callargs_new`, `line`. Function construction (`func_new`, `func_new_closure`,
    `builtin_func`) waits for helper bodies through the builder (typed trampolines).
  - Moved, slice 17 (round 6), on the switch lane's `thread_record_word` and
    `pending::exception_pending_word`: the loaded-method call (the method site's call body is
    `SiteBody::MethodSiteCall`), `DirectCall`'s thread-record handoff around the compiled
    call, and the set and frozenset aggregate. The trait gained `store_word`. LLVM's copies
    and the custody wrappers they used are deleted. The slice's own model tests (round 7):
    `DirectCall` on both machines, where a compiled callee reads the invocation word on entry
    and the outer word comes back; the set aggregate as Cranelift machine code, where each
    entry is boxed only after the previous insertion, a repeated raw value is boxed once, and
    a failed insertion or allocation releases the partial aggregate and every minted box and
    boxes no later entry. A raising runtime stub now sets the thread record's exception hint,
    as the runtime does.
  - Moved, slice 18 (round 7): function construction (`FunctionCx::function_construction_op`:
    `builtin_func`, `func_new`, `func_new_closure`). The trait gained `HelperBody`, a body
    generic over the builder, and `define_helper`, which defines a private helper function once
    by name (LLVM a fresh builder and function state over an internal function; Cranelift a
    second codegen context defined into the module as a local attached function). A helper body
    is generic over the builder the backend makes for it because a helper is another function:
    the caller's builder holds borrows of the caller's function. This replaces the earlier plan of
    an associated helper-builder type with a higher-ranked closure; the generic trait method is
    simpler and needs no lifetime-generic associated type. The callable trampoline (plain and
    builtin) is one helper body: it decodes each boxed argument into its parameter's
    representation, calls the target, ends the references an adopting entry kept only as
    extracted values, and boxes the result. A task constructor's poll and task trampoline stay
    with the switch lane (`ensure_callable_target`); the backend passes them as `task_entry`.
    Trampoline tests run on Cranelift machine code only, entered as the runtime enters them.
- **W3. Cranelift implements the trait and switches.** Delete in the same change: SimpleIR as
  Cranelift input (`lower_to_simple` remains for explicit diagnostic and round-trip consumers), `native_backend`'s op
  lowering, `repr_by_name`, the Cranelift fast-path copies, `list_index_fast_path.rs`. Proof:
  molt_diff basic and stdlib slices on native with no new failure (each one dispositioned);
  instructions retired and wall time on bench_fib, bench_calls, bench_sum, nbody, spectral_norm,
  fannkuch, mandelbrot, bench_method_defaults against main (no regression over 2%, else fixed in
  the late-TIR stage, once); build latency of a user program, cold and warm, no worse; the JIT's
  tests.
  As built, step 1 (2026-11-18, round 5): Cranelift implements the trait
  (`native_backend/codegen_builder.rs`, `NativeCodegen` over `FunctionBuilder`). A truth bit is an
  `I8` holding 0 or 1 (Cranelift has no `i1`, so `bit_not` is `bxor 1`, never `bnot`); an address
  is the target's `I64` and reads back as a word; blocks seal together at finalization; a
  `switch` edge that passes arguments goes through a block of its own, because
  `cranelift_frontend::Switch` targets take none; float `%` calls `fmod` (Cranelift has no
  `frem`); checked add and multiply use the overflow-flag instructions where the ISA lowers them
  and plain instructions on RISC-V 64, which does not. The walker module now compiles for either
  backend. Proof: `codegen_ssa::differential` runs each model test's walker code, written once as
  a `Kernel` generic over the builder, on the interpreting builder and as Cranelift machine code
  compiled by `cranelift_jit`, against one runtime model (stubs for the entries the walker calls);
  `both` requires equal results, build decisions, runtime calls, refcounts reaching the runtime and
  released call inputs. Code that branches and rejoins through block parameters (custody's mint
  and failure paths, branch, conditional branch, switch) runs on the machine alone against its
  oracle, since the interpreter builds every block and cannot follow a parameter. The Cranelift
  verifier also checks every kernel. Production Cranelift task trampolines use the builder.
  Ordinary function bodies still enter the SimpleIR emitter.
- **After W3**: every new fast path is one late-TIR expansion that both native backends get;
  WASM gets it through LLVM wasm32 (the worker lane). Stages 4, 6 and 7 are written once.

### W3 implementation contract

The final-body boundary is `NativeFinalFunctions::from_final_tir`. Today it projects a terminal body into a `FunctionIR` and a name-indexed representation plan, then drops the body. Both native code owners already reach this boundary. Extract the frame, state and task families independently, with LLVM as their production caller, before switching that boundary.

Both native backends use `emit_task_trampoline` for a task constructor invoked through a
callable. The trampoline borrows the closure and argument array and takes its frame context
from the current thread's pending invocation. `TaskPoll` keeps each compiled entry address
with its provider's frame-size relocation. A runtime poll key carries its fixed layout size
and requires no provider data symbol.

1. **One body and one function identity.** Each definition owns its terminal `TirFunction` through emission. Each declaration uses its frozen `NativeFunctionLinkageAbi` and has no executable body. Use the document's `FunctionId` and the existing table/remapping operations for filtering, clones, externalization and partitioning. Recover link names only at object/JIT symbol boundaries. Preserve the roots introduced by module registry data and callable manifests. A body and each reference to it must survive or remap together.
2. **One machine signature.** Caller and provider agree on parameter order, hidden execution context, ownership, and raw, boxed or void return. Use the frozen pre-partition linkage row for both declarations. Convert edge operands before branching with the shared coercion. Walker inputs carry `ValueId`, `BlockId`, `HeldCarrier` and TIR types. A missing definition, return mismatch or illegal opcode ends compilation before an object is finalized or a JIT address is published.
3. **One frame transition per edge.** Synchronous entry uses the existing frame shape and ordered homes. Stateful entry links the persistent activation record on resume and unlinks it on suspension or return. A runtime-linked frame follows its own ABI. Initialize the entry slot to zero and clear it before pop can release homes and re-enter Python. A failed entry has zero base. Preserve handle lifetimes, the exit path for an empty/exceptional return, and the caller's returned owner.
4. **One task layout and resume dispatch.** Resolve a runtime poll key or declared compiled entry and frame-size relocation once from typed identity. Preserve the source task kind, completion mode, captured function/context and payload custody through `TaskConstructorLayout` and the existing task generator. Read the saved state from the activation. State zero uses the default edge, and each resumed edge supplies its own block arguments. Pending-sentinel comparison, exception confirmation and the async-work poll are distinct operations.
5. **One confirmed pending read.** For the semantic operation currently named `emit_exception_pending_word`, zero bypasses the runtime. A nonzero byte requires confirmation from `RuntimeFn::ExceptionPending`; a stale byte can confirm clear. Preserve poll operations that also service async work or fetch an exception payload according to their own contracts.
6. **One production dispatch.** The shared walk covers the admitted final TIR for both native builders. AOT and JIT differ in module allocation, relocation, publication and lifetime. Both invoke the same Cranelift body emitter. Retire each backend-private optimization only after its benefit has been attributed and preserved or disproved on matching inputs. Use exhaustive Rust matches and the actual feature builds to check structural coverage.

For the switch, adopt terminal TIR into the existing final-function owner. Use dense document IDs tied to the final function table. Preserve independently compiled object declarations, imported callables, closure/environment metadata, program slots, module registry relocations and JIT runtime resolution. Thread frozen linkage through module finalization, which currently passes `linkage: None` on the native path. Drop source op arrays as soon as ingest and partition metadata no longer need them. Avoid retaining two stdlib-sized programs. Count emitted terminal operations for diagnostics that describe machine emission. Keep source counts explicitly identified where a public diagnostic measures source operations.

`lower_to_simple_ir_with_names` currently feeds native `ScalarRepresentationPlan::project_final_tir`. Delete that consumer with the switch. `pipeline_cache.rs` also projects bodies for warm and cold SimpleIR requests and partitioning; use the existing TIR-only route and preserve bounded splitting and remapping before terminal drops. The requested rewritten-IR dump in `program_pipeline.rs` and round-trip tests in the pass crates are independent projection consumers. Retain projection where those consumers still request it. The shipped LLVM wasm32 route alone does not establish a WASM machine-code dependency on SimpleIR.

Before deleting the emitter, migrate `simple_backend/tests.rs` and its `tir_analysis` fixtures to
the same final-function boundary as production. Check `tools/proof_plan.toml`'s explicit op-kind,
native-callable ABI and pending-call source paths, the paths in `tools/structural_audit.py`,
`tools/dx_build_timer.py`, `tools/proof_queue_pkg/diagnostic_runtime_rules.py`, the environment
registry owners, and the tests for callable ABI, heap kinds, debug verification and pending calls.
Regenerate the plan and environment projections when their authorities change. The interpreter
in `codegen_ssa/scalar.rs`, the Cranelift machine differential, cross-object linkage tests and
`molt-jit/tests` exercise different consumers of the new boundary.

#### Reviewed Rust boundaries

These signatures describe the accepted ownership boundaries. `B` implements the existing `CodegenBuilder`. The function table retains the declaration ABI for each row. An execution-frame handle is absent when the function owns no frame, and its zero-initialized slot can hold a failed-entry base.

~~~rust
/// I1, I2. Store the definition in the same row as its frozen ABI. The row
/// index is the document FunctionId. Declarations have no machine body.
enum FinalRow {
    Declaration(NativeFunctionLinkageAbi),
    Definition { abi: NativeFunctionLinkageAbi, body: TirFunction },
}

/// I3. The slot holds the currently owned base. None for kind denotes an
/// ABI-linked frame, as in the existing frame-record API.
struct OwnedFrame<S> { slot: S, kind: Option<FrameEntryKind> }

/// I2, I3. Requires the current function's frozen ABI and held carriers.
/// Ensures the result is coerced before retiring the frame and returning.
/// Raises a fatal lowering diagnostic if a value would cross a void ABI.
fn emit_return<B: CodegenBuilder>(
    cx: &mut FunctionCx<'_, B>, b: &mut B, abi: &NativeFunctionLinkageAbi,
    value: Option<ValueId>, frame: Option<&OwnedFrame<B::Slot>>,
) { todo!() }

/// I4. Resolve the typed poll through the current document or runtime
/// manifest. The compiled variant covers a local definition and an import.
/// Only the linkage adapter materializes a symbol/relocation.
enum PollTarget { Runtime(RuntimeFn), Compiled(FunctionId) }

/// I5. Requires the current activation's thread record. Ensures a word of
/// zero or one and confirms every nonzero byte through the runtime reader.
/// Runtime confirmation can clear stale task state.
fn exception_pending_word<B: CodegenBuilder>(b: &mut B, thread: B::Word) -> B::Word {
    todo!()
}

impl SimpleBackend {
    /// I1, I2, I6. Requires complete rows and frozen module declarations.
    /// Ensures verified definitions and metadata precede publication by the
    /// code owner. Raises the existing backend diagnostic on failure.
    fn emit_cranelift_program(
        &mut self, program: NativeCodegenProgram, timing: bool,
    ) -> (usize, usize) { todo!() }
}
~~~

The shared walk and top-level dispatch signatures belong to slot-06. Backend-only strings are restricted to diagnostic text, debug names and symbols at linkage. Machine values keep the builder's associated types. Serialized field names and CLI choices remain boundary syntax. The typed TIR op or canonical ABI layout must carry their decoded meanings to the walker.

#### Qualification

| Obligation | Independent oracle and failure it detects |
|---|---|
| Frame and return | Run existing ABI generator tests and walker machine kernels. Observe nested entry/pop, allocation failure, re-entry during home release, empty and typed returns. Compare Python-visible tracebacks, locals, return values and cleanup with the managed CPython oracle. |
| Tasks and exceptions | Exercise first entry, resume with different edge arguments, send/throw/close, yield-from, cancellation and live locals across suspension. Use a stale nonzero pending byte as well as a real exception and a clear byte. Verify both return value and runtime confirmation count. Use two jobs for async and GC. |
| Linkage and custody | Build producer and caller in separate objects with raw, boxed and void returns and a compiled poll whose spills enlarge its frame. Exercise the JIT loader and owner lifetime. Invalid declarations must fail before publication. Test warm and cold TIR cache inputs, split bodies, clones and extern rows. |
| Family extraction | Compare the 38 release binaries' LLVM text against the same base on the same workload. Where the brief's time exception applies, use interleaved retired instructions and equal output. Exercise model and actual Cranelift kernels and the declared Rust feature cells. |
| Production switch | Run dev/Cranelift basic and stdlib differentials against main, disposition each difference, and run JIT tests. Measure cold and warm user build latency, peak process-tree memory and executable output. Interleave the eight W3 benchmarks against main for instructions and elapsed time. Investigate any regression over 2% in shared lowering before landing. |

The eight workloads are bench_fib, bench_calls, bench_sum, nbody, spectral_norm, fannkuch, mandelbrot and bench_method_defaults. Build base and head at equal-length paths and execute each from the same path. Before deleting Cranelift lowering, obtain the integrator's comparative corpus and investigate each Cranelift advantage, including raise_catch and str_build. Match source/disassembly, a sampled per-module profile and interleaved counts before assigning cause. Record counter and runtime calls per iteration for changed hot paths. Update HF-889 on each implementation landing. These proofs remain outstanding.

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
   options, Cranelift-only `non_escaping_tuple_scalarization`, deleted 2026-11-18: it gated nothing and its consumer had no producer): which are TIR facts the walker
   must honour, and which are backend-private?
4. W3's size: is there a cut of W3 that keeps the no-dual-path rule (for example, the JIT moves
   first, alone, since it is a separate binary), or is W3 one change?
   Answered 2026-11-18. AOT (`simple_backend/compile_driver.rs`) and JIT
   (`simple_backend/jit.rs`) both call `emit_cranelift_program`. A JIT-first change still
   needs the entire walker and would leave a second Cranelift configuration in production.
   Extract families in independently verified LLVM slices. Switch both code owners together
   and delete their shared SimpleIR emitter in that same change.
