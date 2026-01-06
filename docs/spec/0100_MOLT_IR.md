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
Coverage status and planned additions are tracked in `docs/spec/0014_TYPE_COVERAGE_MATRIX.md` with TODO tags for new ops.

## Instruction categories (minimum set)
- **Constants**: `ConstInt`, `ConstFloat`, `ConstBool`, `ConstNone`, `ConstStr`, `ConstBytes`.
- **Arithmetic/logic**: `Add`, `Sub`, `Mul`, `Div`, `Eq`, `Lt`, `Gt`, `Is`, `Contains`, `And`, `Or`, `Not`.
- **Control**: `Phi` (TIR), `Jump`, `Branch`, `Return`, `Throw`, `TryStart`, `TryEnd`, `CheckException`, `LoopStart`, `LoopIndexStart`, `LoopIndexNext`, `LoopBreakIfTrue`, `LoopBreakIfFalse`, `LoopContinue`, `LoopEnd`.
- **Calls**: `Call`, `CallIndirect`, `InvokeFFI` (with declared effects).
- **Object/layout**: `Alloc`, `LoadAttr`, `StoreAttr`, `GetAttrGenericPtr`, `SetAttrGenericPtr`, `GetAttrGenericObj`, `SetAttrGenericObj`, `LoadIndex`, `StoreIndex`, `Index`, `Iter`, `IterNext`, `ListNew`, `DictNew`, `Len`, `Slice`, `SliceNew`, `BytearrayFromObj`, `IntArrayFromSeq`, `MemoryViewNew`, `MemoryViewToBytes`, `RangeNew`, `Buffer2DNew`, `Buffer2DGet`, `Buffer2DSet`, `Buffer2DMatmul`, `ClosureLoad`, `ClosureStore`.
- **Bytes/Bytearray/String**: `BytesFind`, `BytesSplit`, `BytesReplace`, `BytearrayFind`, `BytearraySplit`, `BytearrayReplace`, `StringFind`, `StringFormat`, `StringSplit`, `StringReplace`, `StringStartswith`, `StringEndswith`, `StringCount`, `StringJoin`.
- **Exceptions**: `ExceptionNew`, `ExceptionLast`, `ExceptionClear`, `ExceptionKind`, `ExceptionMessage`, `ExceptionSetCause`, `ExceptionContextSet`, `Raise` (raise sets implicit `__context__`; `ExceptionSetCause` sets explicit `__cause__` and suppresses context).
- **Generators/async**: `AllocGenerator`, `GenSend`, `GenThrow`, `GenClose`, `IsGenerator`, `AIter`, `ANext`, `AllocFuture`, `CallAsync`, `StateSwitch`, `StateTransition`, `StateYield`, `ChanNew`, `ChanSendYield`, `ChanRecvYield`.
  - `StateSwitch` dispatches based on the state slot (`self` payload -16). `StateTransition`/`StateYield` advance the state and return `Pending` when awaiting.
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

## Example (TIR sketch)
```
func add(x: Int, y: Int) -> Int {
  block0(x0: Int, y0: Int):
    v0 = Add x0, y0
    Return v0
}
```
