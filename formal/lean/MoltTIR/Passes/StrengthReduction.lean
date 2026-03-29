/-
  MoltTIR.Passes.StrengthReduction — strength reduction pass on TIR expressions.

  Replaces expensive operations with cheaper algebraic equivalents:
    x * 0 => 0        (annihilation)
    x * 1 => x        (multiplicative identity)
    x + 0 => x        (additive identity)
    x - 0 => x        (subtractive identity)
    x * 2 => x + x    (strength reduction)
    x ** 1 => x        (power identity)
    x ** 0 => 1        (power zero)

  Corresponds to runtime/molt-backend/src/tir/passes/strength_reduction.rs
  in the Molt compiler. The Rust pass currently implements x * 2 => x + x
  and x ** 2 => x * x; this formalization covers the full set of algebraic
  identities that are sound for integer arithmetic.
-/
import MoltTIR.Semantics.EvalExpr

namespace MoltTIR

/-- Apply strength reduction rewrites to an expression. Recursively transforms
    sub-expressions first, then pattern-matches on the top-level form. -/
def srExpr : Expr → Expr
  | .val v => .val v
  | .var x => .var x
  | .bin op a b =>
      let a' := srExpr a
      let b' := srExpr b
      match op, a', b' with
      -- x * 0 => 0  (annihilation)
      | .mul, _, .val (.int 0) => .val (.int 0)
      -- 0 * x => 0  (annihilation, commutative)
      | .mul, .val (.int 0), _ => .val (.int 0)
      -- x * 1 => x  (multiplicative identity)
      | .mul, _, .val (.int 1) => a'
      -- 1 * x => x  (multiplicative identity, commutative)
      | .mul, .val (.int 1), _ => b'
      -- x * 2 => x + x  (strength reduction)
      | .mul, _, .val (.int 2) => .bin .add a' a'
      -- 2 * x => x + x  (strength reduction, commutative)
      | .mul, .val (.int 2), _ => .bin .add b' b'
      -- x + 0 => x  (additive identity)
      | .add, _, .val (.int 0) => a'
      -- 0 + x => x  (additive identity, commutative)
      | .add, .val (.int 0), _ => b'
      -- x - 0 => x  (subtractive identity)
      | .sub, _, .val (.int 0) => a'
      -- x ** 1 => x  (power identity)
      | .pow, _, .val (.int 1) => a'
      -- x ** 0 => 1  (power zero, for all x; matches Python: 0**0 = 1)
      | .pow, _, .val (.int 0) => .val (.int 1)
      -- no rewrite
      | _, _, _ => .bin op a' b'
  | .un op a =>
      .un op (srExpr a)

/-- Apply strength reduction to an instruction. -/
def srInstr (i : Instr) : Instr :=
  { i with rhs := srExpr i.rhs }

/-- Apply strength reduction to a terminator's expressions. -/
def srTerminator : Terminator → Terminator
  | .ret e => .ret (srExpr e)
  | .jmp target args => .jmp target (args.map srExpr)
  | .br cond tl ta el ea =>
      .br (srExpr cond) tl (ta.map srExpr) el (ea.map srExpr)
  | .yield val resume resumeArgs =>
      .yield (srExpr val) resume (resumeArgs.map srExpr)
  | .switch scrutinee cases default_ =>
      .switch (srExpr scrutinee) cases default_
  | .unreachable => .unreachable

/-- Apply strength reduction to a block. -/
def srBlock (b : Block) : Block :=
  { b with
    instrs := b.instrs.map srInstr
    term := srTerminator b.term }

/-- Apply strength reduction to a function (all blocks via blockList). -/
def srFunc (f : Func) : Func :=
  { f with blockList := f.blockList.map fun (lbl, blk) => (lbl, srBlock blk) }

end MoltTIR
