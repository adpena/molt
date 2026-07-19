/-
  MoltTIR.Semantics.EvalExpr — expression evaluation (pure, deterministic).

  Covers Molt's core arithmetic, comparison, and bitwise opcodes.
  Additional opcodes (string ops, collection ops, etc.) are modeled
  as opaque intrinsics that don't need expression-level semantics.
-/
import MoltTIR.Semantics.State

namespace MoltTIR

/-- Identity for the scalar Value universe where source provenance is sufficient.
    Bool and None are singletons and distinct constructors are disjoint. Equal
    int, float, and string payloads do not prove object identity, so those
    same-kind cases deliberately remain undefined. -/
def scalarIdentityEq : Value → Value → Option Bool
  | .bool x, .bool y => some (x == y)
  | .none, .none => some true
  | .int _, .int _ | .float _, .float _ | .str _, .str _ => none
  | _, _ => some false

/-- Provenance classes consumed by the backend identity-admission table. The
    first five constructors are the scalar subset represented by `Value`;
    references and unknowns model the wider emitted-IR boundary explicitly. -/
inductive IdentityProvenance where
  | singletonBool | singletonNone
  | valueInt | valueFloat | valueStr
  | reference | unknown
  deriving DecidableEq, Repr

/-- Exact backend action for a Python identity comparison. -/
inductive IdentityLowering where
  | constant (value : Bool)
  | directRawEqual
  | reject
  deriving DecidableEq, Repr

def scalarIdentityProvenance : Value → IdentityProvenance
  | .bool _ => .singletonBool
  | .none => .singletonNone
  | .int _ => .valueInt
  | .float _ => .valueFloat
  | .str _ => .valueStr

/-- Formal counterpart of the Rust `identity_lowering_for_provenance` table.
    `directRawEqual` means the trusted Luau `rawequal` primitive, never `==`. -/
def identityLowering (sameSsa : Bool) : IdentityProvenance → IdentityProvenance → IdentityLowering
  | lhs, rhs => if sameSsa then .constant true else match lhs, rhs with
    | .valueInt, .valueInt | .valueFloat, .valueFloat | .valueStr, .valueStr => .reject
    | .valueInt, .unknown | .valueFloat, .unknown | .valueStr, .unknown
    | .unknown, .valueInt | .unknown, .valueFloat | .unknown, .valueStr
    | .unknown, .unknown => .reject
    | .singletonBool, .singletonBool | .singletonNone, .singletonNone => .directRawEqual
    | .reference, .reference | .reference, .unknown | .unknown, .reference
    | .singletonBool, .unknown | .singletonNone, .unknown
    | .unknown, .singletonBool | .unknown, .singletonNone => .directRawEqual
    | _, _ => .constant false

/-- SSA self-identity dominates every provenance class. -/
@[simp] theorem identityLowering_sameSsa (lhs rhs : IdentityProvenance) :
    identityLowering true lhs rhs = .constant true := by
  rfl

def identityLoweringOutcome (lowering : IdentityLowering) (lhs rhs : Value) : Option Bool :=
  match lowering with
  | .constant value => some value
  | .directRawEqual => scalarIdentityEq lhs rhs
  | .reject => none

/-- The admitted scalar-provenance table is extensionally identical to the
    source identity semantics for distinct SSA names. -/
@[simp] theorem identityLowering_scalar_correspondence (lhs rhs : Value) :
    identityLoweringOutcome
        (identityLowering false (scalarIdentityProvenance lhs) (scalarIdentityProvenance rhs))
        lhs rhs = scalarIdentityEq lhs rhs := by
  cases lhs <;> cases rhs <;> rfl

/-- Counted 7x7 admission receipt. The correspondence checker compares this
    kernel-checked matrix with the Rust matrix exercised by backend tests. -/
theorem identityLowering_complete_matrix :
    [
      identityLowering false .singletonBool .singletonBool,
      identityLowering false .singletonBool .singletonNone,
      identityLowering false .singletonBool .valueInt,
      identityLowering false .singletonBool .valueFloat,
      identityLowering false .singletonBool .valueStr,
      identityLowering false .singletonBool .reference,
      identityLowering false .singletonBool .unknown,
      identityLowering false .singletonNone .singletonBool,
      identityLowering false .singletonNone .singletonNone,
      identityLowering false .singletonNone .valueInt,
      identityLowering false .singletonNone .valueFloat,
      identityLowering false .singletonNone .valueStr,
      identityLowering false .singletonNone .reference,
      identityLowering false .singletonNone .unknown,
      identityLowering false .valueInt .singletonBool,
      identityLowering false .valueInt .singletonNone,
      identityLowering false .valueInt .valueInt,
      identityLowering false .valueInt .valueFloat,
      identityLowering false .valueInt .valueStr,
      identityLowering false .valueInt .reference,
      identityLowering false .valueInt .unknown,
      identityLowering false .valueFloat .singletonBool,
      identityLowering false .valueFloat .singletonNone,
      identityLowering false .valueFloat .valueInt,
      identityLowering false .valueFloat .valueFloat,
      identityLowering false .valueFloat .valueStr,
      identityLowering false .valueFloat .reference,
      identityLowering false .valueFloat .unknown,
      identityLowering false .valueStr .singletonBool,
      identityLowering false .valueStr .singletonNone,
      identityLowering false .valueStr .valueInt,
      identityLowering false .valueStr .valueFloat,
      identityLowering false .valueStr .valueStr,
      identityLowering false .valueStr .reference,
      identityLowering false .valueStr .unknown,
      identityLowering false .reference .singletonBool,
      identityLowering false .reference .singletonNone,
      identityLowering false .reference .valueInt,
      identityLowering false .reference .valueFloat,
      identityLowering false .reference .valueStr,
      identityLowering false .reference .reference,
      identityLowering false .reference .unknown,
      identityLowering false .unknown .singletonBool,
      identityLowering false .unknown .singletonNone,
      identityLowering false .unknown .valueInt,
      identityLowering false .unknown .valueFloat,
      identityLowering false .unknown .valueStr,
      identityLowering false .unknown .reference,
      identityLowering false .unknown .unknown
    ] =
    [
      .directRawEqual,
      .constant false,
      .constant false,
      .constant false,
      .constant false,
      .constant false,
      .directRawEqual,
      .constant false,
      .directRawEqual,
      .constant false,
      .constant false,
      .constant false,
      .constant false,
      .directRawEqual,
      .constant false,
      .constant false,
      .reject,
      .constant false,
      .constant false,
      .constant false,
      .reject,
      .constant false,
      .constant false,
      .constant false,
      .reject,
      .constant false,
      .constant false,
      .reject,
      .constant false,
      .constant false,
      .constant false,
      .constant false,
      .reject,
      .constant false,
      .reject,
      .constant false,
      .constant false,
      .constant false,
      .constant false,
      .constant false,
      .directRawEqual,
      .directRawEqual,
      .directRawEqual,
      .directRawEqual,
      .reject,
      .reject,
      .reject,
      .directRawEqual,
      .reject
    ] := by
  rfl


/-- Evaluate a binary operator on two values. Returns none on type mismatch. -/
def evalBinOp (op : BinOp) (a b : Value) : Option Value :=
  match op, a, b with
  -- arithmetic (int × int → int)
  | .add, .int x, .int y => some (.int (x + y))
  | .sub, .int x, .int y => some (.int (x - y))
  | .mul, .int x, .int y => some (.int (x * y))
  | .mod, .int x, .int y => if y == 0 then none else some (.int (x % y))
  | .floordiv, .int x, .int y => if y == 0 then none else some (.int (x / y))
  | .pow, .int x, .int y =>
      if y < 0 then none
      else some (.int (x ^ y.toNat))
  -- string concatenation
  | .add, .str x, .str y => some (.str (x ++ y))
  -- string repetition (str * int)
  | .mul, .str s, .int n =>
      if n ≤ 0 then some (.str "")
      else some (.str (String.join (List.replicate n.toNat s)))
  | .mul, .int n, .str s =>
      if n ≤ 0 then some (.str "")
      else some (.str (String.join (List.replicate n.toNat s)))
  -- int * float promotion
  | .add, .int x, .float y => some (.float (x + y))
  | .sub, .int x, .float y => some (.float (x - y))
  | .mul, .int x, .float y => some (.float (x * y))
  | .add, .float x, .int y => some (.float (x + y))
  | .sub, .float x, .int y => some (.float (x - y))
  | .mul, .float x, .int y => some (.float (x * y))
  -- float * float arithmetic
  | .add, .float x, .float y => some (.float (x + y))
  | .sub, .float x, .float y => some (.float (x - y))
  | .mul, .float x, .float y => some (.float (x * y))
  -- comparison (int × int → bool)
  | .eq,  .int x, .int y => some (.bool (x == y))
  | .ne,  .int x, .int y => some (.bool (x != y))
  | .lt,  .int x, .int y => some (.bool (x < y))
  | .le,  .int x, .int y => some (.bool (x ≤ y))
  | .gt,  .int x, .int y => some (.bool (x > y))
  | .ge,  .int x, .int y => some (.bool (x ≥ y))
  -- comparison across the complete numeric carrier family
  | .eq,  .int x, .float y => some (.bool (x == y))
  | .ne,  .int x, .float y => some (.bool (x != y))
  | .lt,  .int x, .float y => some (.bool (x < y))
  | .le,  .int x, .float y => some (.bool (x ≤ y))
  | .gt,  .int x, .float y => some (.bool (x > y))
  | .ge,  .int x, .float y => some (.bool (x ≥ y))
  | .eq,  .float x, .int y => some (.bool (x == y))
  | .ne,  .float x, .int y => some (.bool (x != y))
  | .lt,  .float x, .int y => some (.bool (x < y))
  | .le,  .float x, .int y => some (.bool (x ≤ y))
  | .gt,  .float x, .int y => some (.bool (x > y))
  | .ge,  .float x, .int y => some (.bool (x ≥ y))
  | .eq,  .float x, .float y => some (.bool (x == y))
  | .ne,  .float x, .float y => some (.bool (x != y))
  | .lt,  .float x, .float y => some (.bool (x < y))
  | .le,  .float x, .float y => some (.bool (x ≤ y))
  | .gt,  .float x, .float y => some (.bool (x > y))
  | .ge,  .float x, .float y => some (.bool (x ≥ y))
  -- comparison (bool × bool → bool)
  | .eq,  .bool x, .bool y => some (.bool (x == y))
  | .ne,  .bool x, .bool y => some (.bool (x != y))
  -- Boolean operators preserve Python's operand-returning semantics on the
  -- scalar Value universe. Identity is structural identity for these values.
  | .and_, .bool false, _ => some (.bool false)
  | .and_, .bool true, y => some y
  | .or_, .bool true, _ => some (.bool true)
  | .or_, .bool false, y => some y
  | .is, x, y => (scalarIdentityEq x y).map .bool
  | .is_not, x, y => (scalarIdentityEq x y).map (fun same => .bool (!same))
  -- bitwise ops (bit_and, bit_or, bit_xor, lshift, rshift) are defined in
  -- the syntax but not evaluated here — they fall to the catch-all.
  -- Lean's Int lacks HAnd/HOr/HXor; add implementations when needed.
  -- catch-all for type mismatches, unmodeled ops, and bitwise
  | _, _, _ => none

/-- Evaluate a unary operator.
    `not` applies to all scalar types via truthy coercion (matching
    Python's `not` semantics): bool→bool negation, int→bool (nonzero test),
    float→bool (nonzero test), str→bool (nonempty test), none→true. -/
def evalUnOp (op : UnOp) (a : Value) : Option Value :=
  match op, a with
  | .neg, .int x => some (.int (-x))
  | .neg, .float x => some (.float (-x))
  | .not, .bool x => some (.bool (!x))
  | .not, .int x => some (.bool (x == 0))
  | .not, .float x => some (.bool (x == 0))
  | .not, .str s => some (.bool (s == ""))
  | .not, .none => some (.bool true)
  | .abs, .int x => some (.int (if x < 0 then -x else x))
  | .pos, .int x => some (.int x)
  | .pos, .float x => some (.float x)
  | _, _ => none

/-- Evaluate an expression in an environment. Total, deterministic. -/
def evalExpr (ρ : Env) : Expr → Option Value
  | .val v => some v
  | .var x => ρ x
  | .bin op a b =>
      match evalExpr ρ a, evalExpr ρ b with
      | some va, some vb => evalBinOp op va vb
      | _, _ => none
  | .un op a =>
      match evalExpr ρ a with
      | some va => evalUnOp op va
      | none => none

/-- Evaluating an expression in an environment extended with an irrelevant
    binding produces the same result. This is the key lemma for DCE correctness:
    if x does not appear in e, then setting x in ρ does not affect evalExpr ρ e. -/
theorem evalExpr_set_irrelevant (ρ : Env) (x : Var) (v : Value) (e : Expr)
    (h : x ∉ exprVars e) : evalExpr (ρ.set x v) e = evalExpr ρ e := by
  induction e with
  | val _ => rfl
  | var y =>
    have hne : y ≠ x := by
      intro heq; apply h; simp [exprVars]; exact heq.symm
    exact Env.set_ne ρ x y v hne
  | bin op a b iha ihb =>
    have ha : x ∉ exprVars a := fun hm => h (by simp [exprVars]; exact Or.inl hm)
    have hb : x ∉ exprVars b := fun hm => h (by simp [exprVars]; exact Or.inr hm)
    simp only [evalExpr, iha ha, ihb hb]
  | un op a iha =>
    simp only [evalExpr, iha h]

/-- evalExpr is a function, so it is trivially deterministic. -/
theorem evalExpr_deterministic (ρ : Env) (e : Expr) :
    ∀ v1 v2, evalExpr ρ e = some v1 → evalExpr ρ e = some v2 → v1 = v2 := by
  intro v1 v2 h1 h2
  simp [h1] at h2
  exact h2

end MoltTIR
