/-
  MoltTIR.Passes.BCE — bounds check elimination pass on TIR.

  Marks `index` operations that are provably bounds-check-safe by annotating
  them with a `bce_safe` flag.  Downstream codegen can test for this flag
  and skip the runtime bounds check.

  ## Current scope (Phase 2 — constant-index BCE)

  An `index` op is marked safe when **all** of the following hold:
    1. The index expression is a constant integer (`Expr.val (Value.int n)`).
    2. The constant value is **non-negative** (`n ≥ 0`).

  Negative constant indices still require a runtime wraparound (Python
  semantics `lst[-1]`), so they are intentionally left unmarked.

  Corresponds to runtime/molt-backend/src/tir/passes/bce.rs.
-/
import MoltTIR.Syntax

namespace MoltTIR

-- ---------------------------------------------------------------------------
-- Annotated side-effect operation
-- ---------------------------------------------------------------------------

/-- A side-effecting operation paired with an optional `bce_safe` annotation.
    When `bce_safe = true`, codegen may elide the runtime bounds check. -/
structure AnnotatedOp where
  op       : SideEffectOp
  bce_safe : Bool := false
  deriving Repr

/-- Predicate: the index expression of an `index` op is a non-negative constant. -/
def isConstNonNegIndex : SideEffectOp → Bool
  | .index _ (.val (.int n)) _ => n ≥ 0
  | _                          => false

/-- Apply BCE to a single side-effect operation.
    If the op is an `index` with a constant non-negative index expression,
    return an `AnnotatedOp` with `bce_safe = true`; otherwise leave it unmarked. -/
def bceOp (op : SideEffectOp) : AnnotatedOp :=
  { op := op, bce_safe := isConstNonNegIndex op }

/-- Apply BCE to a list of side-effect operations (one block's worth). -/
def bceOps (ops : List SideEffectOp) : List AnnotatedOp :=
  ops.map bceOp

-- ---------------------------------------------------------------------------
-- Block / function-level lifting (Phase 2 — operates on side-effect lists)
-- ---------------------------------------------------------------------------

/-- A block extended with annotated side-effect operations.
    The pure `instrs` and `term` are unchanged by BCE. -/
structure AnnotatedBlock where
  params    : List Var
  instrs    : List Instr
  sideEffs  : List AnnotatedOp
  term      : Terminator
  deriving Repr

/-- Annotate a block's side-effect operations with BCE flags. -/
def bceBlock (b : Block) (sideEffs : List SideEffectOp) : AnnotatedBlock :=
  { params   := b.params
    instrs   := b.instrs
    sideEffs := bceOps sideEffs
    term     := b.term }

-- ---------------------------------------------------------------------------
-- Convenience: count how many ops were marked safe (mirrors PassStats)
-- ---------------------------------------------------------------------------

/-- Count the number of ops marked `bce_safe` in an annotated list. -/
def bceCount (ops : List AnnotatedOp) : Nat :=
  ops.filter (·.bce_safe) |>.length

end MoltTIR
