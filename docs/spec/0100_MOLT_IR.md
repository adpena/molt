# Molt IR

Molt IR is typed SSA with explicit control flow, ownership, and effects. It exists in multiple layers (HIR, TIR, LIR), each enforcing stronger invariants to enable aggressive optimization while preserving soundness.

## IR stack
- **HIR (High-level IR)**: desugared AST with explicit control flow.
- **TIR (Typed IR)**: SSA with concrete `MoltType` for every value.
- **LIR (Low-level IR)**: explicit memory operations, reference counting, and layout-level access.

## Core structure
- **Module**: a set of functions, globals, and type definitions.
- **Function**: SSA values, basic blocks, and a declared effect summary.
- **Block**: parameters + instruction list + terminator.
- **Terminator**: `Jump`, `Branch`, `Return`, `Throw`.

## Type system (minimum set)
- **Primitives**: `Int`, `Float`, `Bool`, `None`.
- **Objects**: `Class(Id)`, `List(T)`, `Dict(K,V)`, `Tuple([...])`, `Str`, `Bytes`.
- **Unions**: `Union(T1, T2, ...)`, `Any` (Tier 1 only).

## Instruction categories (minimum set)
- **Constants**: `ConstInt`, `ConstFloat`, `ConstBool`, `ConstNone`, `ConstStr`.
- **Arithmetic/logic**: `Add`, `Sub`, `Mul`, `Div`, `Eq`, `Lt`, `Gt`, `And`, `Or`, `Not`.
- **Control**: `Phi` (TIR), `Jump`, `Branch`, `Return`, `Throw`.
- **Calls**: `Call`, `CallIndirect`, `InvokeFFI` (with declared effects).
- **Object/layout**: `Alloc`, `LoadAttr`, `StoreAttr`, `LoadIndex`, `StoreIndex`.
- **Guards (Tier 1)**: `GuardType`, `GuardTag`, `GuardLayout`, `GuardDictShape`.
- **RC ops (LIR)**: `IncRef`, `DecRef`, `Borrow`, `Release`.
- **Conversions**: `Box`, `Unbox`, `Cast`, `Widen`.

## Invariants
- **SSA**: every value is defined once; all uses are dominated by the definition.
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
