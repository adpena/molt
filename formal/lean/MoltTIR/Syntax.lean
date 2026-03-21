/-
  MoltTIR.Syntax — IR abstract syntax for the Molt TIR core.

  This models the actual Molt IR structure:
  - A function is a named entity with params and a list of operations (MoltOp)
  - Each op has a kind (opcode), arguments (input SSA vars), and an output var
  - Control flow uses if/else/end_if and label/jump (implicit CFG)

  For formal verification, we keep a block-parameter SSA abstraction
  that captures the *semantics* in a proof-friendly way, while documenting
  the correspondence to the real flat-opcode representation.

  Real Molt IR structures (for reference):
    Python: MoltOp { kind: str, args: list, result: MoltValue }
    Rust:   OpIR { kind: String, value: Option<i64>, args: Option<Vec<String>>, out: Option<String> }
-/
import MoltTIR.Types

namespace MoltTIR

abbrev Var := Nat
abbrev Label := Nat

/-- Runtime values. Corresponds to Molt's NaN-boxed value representation. -/
inductive Value where
  | int (n : Int)
  | bool (b : Bool)
  | float (f : Int)     -- model floats as fixed-point for determinism proofs
  | str (s : String)
  | none
  deriving DecidableEq, Repr

/-- Binary operators. Maps to Molt opcodes: add, sub, mul, div, floordiv, mod, pow,
    eq, ne, lt, le, gt, ge, bit_and, bit_or, bit_xor, lshift, rshift. -/
inductive BinOp where
  -- arithmetic
  | add | sub | mul | div | floordiv | mod | pow
  -- comparison
  | eq | ne | lt | le | gt | ge
  -- bitwise
  | bit_and | bit_or | bit_xor | lshift | rshift
  deriving DecidableEq, Repr

/-- Unary operators. Maps to Molt opcodes: not, abs, neg, invert. -/
inductive UnOp where
  | neg       -- arithmetic negation
  | not       -- boolean negation
  | abs       -- absolute value
  | invert    -- bitwise inversion
  deriving DecidableEq, Repr

/-- Expressions (pure, no side effects). -/
inductive Expr where
  | val (v : Value)
  | var (x : Var)
  | bin (op : BinOp) (a b : Expr)
  | un  (op : UnOp) (a : Expr)
  deriving DecidableEq, Repr

/-- SSA instruction: assign `dst` := `rhs`. dst is fresh in SSA.
    Corresponds to a MoltOp with out=dst and evaluated rhs.

    fast_int_hint / fast_float_hint: optional type specialization flags propagated
    from type inference. When true, the backend may emit specialized integer or
    float arithmetic without a type-tag check. Added to match implementation:
      2e1cab40 perf: propagate type facts to IR fast_int/fast_float flags
      14ad1fe3 perf: automatic int type inference from range/len/literals -/
structure Instr where
  dst : Var
  rhs : Expr
  fast_int_hint   : Bool := false
  fast_float_hint : Bool := false
  deriving Repr

/-- Block terminators with explicit argument passing (block params).
    In the real Molt IR these are represented as if/else/end_if + label/jump opcodes;
    the block-parameter form is a proof-friendly abstraction of the same semantics.

    `yield` models the STATE_YIELD opcode used by generator/coroutine lowering.
    It suspends execution, yielding `val` to the caller, and resumes at `resume`
    when the generator is next iterated. Matches the implementation in
    src/molt/frontend/cfg_analysis.py (STATE_YIELD as a block terminator). -/
inductive Terminator where
  | ret (e : Expr)
  | jmp (target : Label) (args : List Expr)
  | br  (cond : Expr)
       (thenLabel : Label) (thenArgs : List Expr)
       (elseLabel : Label) (elseArgs : List Expr)
  | yield (val : Expr) (resume : Label) (resumeArgs : List Expr)
  deriving Repr

/-- A basic block with parameters, instructions, and a terminator.
    Corresponds to a contiguous range of MoltOps between control flow boundaries. -/
structure Block where
  params : List Var
  instrs : List Instr
  term   : Terminator
  deriving Repr

/-- A function: an entry label and a finite map from labels to blocks.
    Corresponds to FunctionIR { name, params, ops } in the real IR.
    We use a List-backed map rather than a bare function to enable Repr. -/
structure Func where
  entry  : Label
  blockList : List (Label × Block)

namespace Func

def blocks (f : Func) (lbl : Label) : Option Block :=
  match f.blockList.find? (fun p => p.1 == lbl) with
  | some (_, b) => some b
  | none => none

end Func

/-- Collect all variables referenced in an expression. -/
def exprVars : Expr → List Var
  | .val _ => []
  | .var x => [x]
  | .bin _ a b => exprVars a ++ exprVars b
  | .un _ a => exprVars a

/-- Collect all variables referenced in a terminator. -/
def termVars : Terminator → List Var
  | .ret e => exprVars e
  | .jmp _ args => args.bind exprVars
  | .br cond _ thenArgs _ elseArgs =>
      exprVars cond ++ thenArgs.bind exprVars ++ elseArgs.bind exprVars
  | .yield val _ resumeArgs =>
      exprVars val ++ resumeArgs.bind exprVars

end MoltTIR
