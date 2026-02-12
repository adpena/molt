# Molt IR

Molt IR is typed SSA with explicit control flow, ownership, and effects. It exists in multiple layers (HIR, TIR, LIR), each enforcing stronger invariants to enable aggressive optimization while preserving soundness.

## IR stack
- **HIR (High-level IR)**: desugared AST with explicit control flow.
- **TIR (Typed IR)**: SSA with concrete `MoltType` for every value.
- **LIR (Low-level IR)**: explicit memory operations, reference counting, and layout-level access.

## Implementation layout
- Keep Rust crate entrypoints (`lib.rs`) thin; implement runtime/backend subsystems in focused modules under `src/` and re-export from `lib.rs`.

## Core structure
- **Module**: a set of functions, globals, and type definitions.
- **Function**: SSA values, basic blocks, and a declared effect summary.
- **Block**: parameters + instruction list + terminator.
- **Terminator**: `Jump`, `Branch`, `Return`, `Throw`.

## Type system (minimum set)
- **Primitives**: `Int`, `Float`, `Bool`, `None`.
- **Objects**: `Class(Id)`, `List(T)`, `Dict(K,V)`, `Tuple([...])`, `Str`, `Bytes`, `MemoryView`, `Range`, `Slice`.
- **Unions**: `Union(T1, T2, ...)`, `Any` (Tier 1 only).
Coverage status and planned additions are tracked in `docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md` with TODO tags for new ops.

## Instruction categories (minimum set)
- **Constants**: `ConstInt`, `ConstFloat`, `ConstBool`, `ConstNone`, `ConstStr`, `ConstBytes`.
- **Arithmetic/logic**: `Add`, `Sub`, `Mul`, `Div`, `Eq`, `Lt`, `Gt`, `Is`, `Contains`, `And`, `Or`, `Not`.
- **Control**: `Phi` (TIR), `Jump`, `Branch`, `Return`, `Throw`, `TryStart`, `TryEnd`, `CheckException`, `LoopStart`, `LoopIndexStart`, `LoopIndexNext`, `LoopBreakIfTrue`, `LoopBreakIfFalse`, `LoopBreak`, `LoopContinue`, `LoopEnd`.
- **Calls**: `Call`, `CallIndirect`, `InvokeFFI` (with declared effects).
- **Object/layout**: `Alloc`, `LoadAttr`, `StoreAttr`, `GetAttrGenericPtr`, `SetAttrGenericPtr`, `GetAttrGenericObj`, `SetAttrGenericObj`, `LoadIndex`, `StoreIndex`, `Index`, `Iter`, `Enumerate`, `IterNext`, `ListNew`, `DictNew`, `Len`, `Slice`, `SliceNew`, `BytearrayFromObj`, `IntArrayFromSeq`, `MemoryViewNew`, `MemoryViewToBytes`, `RangeNew`, `Buffer2DNew`, `Buffer2DGet`, `Buffer2DSet`, `Buffer2DMatmul`, `ClosureLoad`, `ClosureStore`.
- **Bytes/Bytearray/String**: `BytesFind`, `BytesSplit`, `BytesReplace`, `BytearrayFind`, `BytearraySplit`, `BytearrayReplace`, `StringFind`, `StringFormat`, `StringSplit`, `StringCapitalize`, `StringStrip`, `StringReplace`, `StringStartswith`, `StringEndswith`, `StringCount`, `StringJoin`.
- **Exceptions**: `ExceptionNew`, `ExceptionLast`, `ExceptionClear`, `ExceptionKind`, `ExceptionMessage`, `ExceptionSetCause`, `ExceptionContextSet`, `Raise` (raise sets implicit `__context__`; `ExceptionSetCause` sets explicit `__cause__` and suppresses context).
- **Generators/async**: `AllocGenerator`, `GenSend`, `GenThrow`, `GenClose`, `IsGenerator`, `AIter`, `ANext`, `AllocFuture`, `CallAsync`, `StateSwitch`, `StateTransition`, `StateYield`, `ChanNew`, `ChanSendYield`, `ChanRecvYield`.
  - `StateSwitch` dispatches based on the state slot (`self` payload -16). `StateTransition`/`StateYield` advance the state and return `Pending` when awaiting.
  - Implementations may encode resume targets in the state slot (for example,
    bitwise NOT of the resume op index) to avoid collisions with logical state
    ids; `StateSwitch` must decode before dispatch.
  - `ChanNew` takes a boxed capacity; `0` creates an unbounded channel.
  - `ChanSendYield`/`ChanRecvYield` advance the state and return `Pending` when channel operations suspend, otherwise they yield the send/recv result immediately.
- **Vector**: `VecSumInt`, `VecProdInt`, `VecMinInt`, `VecMaxInt` (guarded reductions; emit `(result, ok)` tuples), plus trusted variants (`VecSumIntTrusted`, `VecProdIntTrusted`, `VecMinIntTrusted`, `VecMaxIntTrusted`) that skip per-element checks when type facts are trusted. Range-aware variants (`Vec*IntRange`, `Vec*IntRangeTrusted`) accept a start offset for `range(k, len(xs))` patterns.
- **Guards (Tier 1)**: `GuardType`, `GuardTag`, `GuardLayout`, `GuardDictShape`.
- **RC ops (LIR)**: `IncRef`, `DecRef`, `Borrow`, `Release`.
- **Conversions**: `Box`, `Unbox`, `Cast`, `Widen`, `StrFromObj`.

## Invariants
- **SSA**: every value is defined once; all uses are dominated by the definition (loop index carried via block params).
- **Explicit effects**: calls and memory ops must declare their effect class.
- **No implicit exceptions**: operations that can fail must either be guarded or emit `Throw`.
- **Tier separation**: Tier 0 disallows `Any` and speculative guards; Tier 1 allows guards with deopt exits.

## Implementation Status Snapshot (2026-02-11)
- Implemented in this repo today:
  - `SimpleTIRGenerator` in `src/molt/frontend/__init__.py` emits a broad TIR
    op surface used by `molt build`/`molt run`.
- Detailed instruction-by-instruction audit:
  - `docs/spec/areas/compiler/0100_MOLT_IR_IMPLEMENTATION_COVERAGE_2026-02-11.md`.
- Partial / pending:
  - Dedicated HIR and LIR modules are not yet split into standalone compiler
    crates/modules in this tree.
  - Frontend lowering now includes a lightweight mid-end canonicalization
    pipeline before JSON IR serialization (check-exception coalescing +
    explicit basic-block CFG, dominator, and liveness passes). Current
    behavior includes deterministic fixed-point ordering
    (`simplify -> SCCP -> canonicalize -> DCE`) with sparse SCCP lattice
    propagation (`unknown`/`constant`/`overdefined`) over SSA names, explicit
    executable-edge tracking, SCCP folding for arithmetic/boolean/comparison/
    `TypeOf` plus constant-safe `Contains`/`Index`, selected `IsInstance`
    folds, and selected guard-tag/dict-shape facts (including guard-failure
    edge termination). It now tracks try exceptional vs normal completion facts
    and threads executable edges for `If`/`LoopBreakIf*`/`LoopEnd`/`Try*`, and
    region-aware CFG
    simplification across structured `If`/`Else`, `Loop`, `Try`, and
    `Label`/`Jump` regions (including dead-label pruning and no-op jump
    elimination, plus dead try-body suffix pruning after proven guard/raise
    exits). A structural canonicalization step now runs before SCCP each round
    to remove degenerate empty branch/loop/try regions. It also includes
    conservative branch-tail merging, loop-invariant pure-op hoisting,
    pure/read-heap cross-block CSE with conservative alias/effect classes
    (`dict`/`list`/`indexable`/`attr`) including `GetAttr`/`LoadAttr`/`Index`
    reuse under no-interfering-write checks. Read-heap invalidation treats
    call/invoke operations as conservative write barriers, and class-level
    alias epochs are augmented with lightweight object-sensitive epochs for
    higher CSE hit-rate without unsafe reuse. Exceptional try-edge pruning
    preserves balanced `TryStart`/`TryEnd` structure unless
    dominance/post-dominance plus pre-trap `CheckException`-free proofs permit
    marker elision. The CFG now models explicit `CheckException` branch targets
    and threads proven exceptional checks into direct handler `Jump` edges with
    dominance-safe guards before unreachable-region pruning. It also normalizes
    nested try/except multi-handler join trampolines (label->jump chains)
    before CSE rounds, and
    side-effect-aware DCE with strict protection for guard/call/exception/control
    ops. Expanded cross-block value reuse is still guarded by a CFG
    definite-assignment verifier with automatic safe fallback when proof fails.
    Loop analysis tracks `(start, step, bound, compare-op)` tuples for affine
    induction facts and monotonic bound proofs used by SCCP.
    The pass also performs trivial-`Phi` elision, proven no-op `GuardTag`
    elision, and dominance-safe hoisting of duplicate branch guards across
    structured joins. CFG analysis data structures are now first-class in
    `src/molt/frontend/cfg_analysis.py`, and mid-end telemetry reports
    per-transform and function-scoped counters via `MOLT_MIDEND_STATS=1`.
  - Frontend/lowering/backend now provide dedicated lanes for
    `CallIndirect`/`InvokeFFI`/`GuardTag`/`GuardDictShape` and
    `IncRef`/`DecRef`/`Borrow`/`Release` plus conversion families
    (`Box`/`Unbox`/`Cast`/`Widen`); deterministic semantic-depth hardening and
    broader differential evidence are still in progress.
  - `tools/check_molt_ir_ops.py` now enforces inventory coverage,
    dedicated-lane presence, and behavior-level semantic assertions for these
    lanes across frontend/native/wasm (including dedicated native+wasm call-site
    labels for `invoke_ffi_bridge`/`invoke_ffi_deopt` vs `call_func` and
    `call_indirect` vs `call_bind`).
  - LIR-level explicit RC ops (`IncRef`/`DecRef`/`Borrow`/`Release`) are
    specified here but not fully materialized as a separate lowering stage in
    the frontend emitter.
- Tracking policy:
  - Treat this spec as the contract; implementation status remains canonical in
    `docs/spec/STATUS.md` and `ROADMAP.md`.

## Month 1 Sign-off Readiness
- Status: Draft ready for alignment review (2026-02-11) per `docs/ROADMAP_90_DAYS.md`.
- Criteria:
  1. IR stack and instruction categories stay aligned with emitted ops in
     `src/molt/frontend/__init__.py`.
  2. Any intentional gaps (for example HIR/LIR split and RC-lowering stage) are
     reflected in `docs/spec/STATUS.md` and `ROADMAP.md`.
  3. Differential/semantic gate ownership remains tied to
     `docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md`.
  4. Compiler/runtime owners review and acknowledge this spec revision.
- Sign-off date: pending explicit owner approval (candidate baseline: 2026-02-11).

## Example (TIR sketch)
```
func add(x: Int, y: Int) -> Int {
  block0(x0: Int, y0: Int):
    v0 = Add x0, y0
    Return v0
}
```
