/-
  MoltTIR.TypeSystem.TypeInference — type inference algorithm for MoltTIR.

  Defines `inferType`, a decidable type inference function over MoltTIR
  expressions, and proves:
  - Soundness: if `inferType Γ e = some τ` then `HasType Γ e τ`.
  - Completeness: for literal and variable cases, `HasType Γ e τ`
    implies `inferType Γ e = some τ`.
-/
import MoltTIR.TypeSystem.TypeSoundness

set_option autoImplicit false

namespace MoltTIR

/-! ## Type inference for binary operators -/

/-- Infer the result type of a binary operator given operand types.
    Returns `none` for unsupported type combinations. -/
def inferBinOpTy (op : BinOp) (lhs rhs : Ty) : Option Ty :=
  match op, lhs, rhs with
  -- arithmetic: int × int → int
  | .add, .int, .int => some .int
  | .sub, .int, .int => some .int
  | .mul, .int, .int => some .int
  | .mod, .int, .int => some .int
  -- comparison: int × int → bool
  | .eq,  .int, .int => some .bool
  | .ne,  .int, .int => some .bool
  | .lt,  .int, .int => some .bool
  | .le,  .int, .int => some .bool
  | .gt,  .int, .int => some .bool
  | .ge,  .int, .int => some .bool
  -- comparison: bool × bool → bool
  | .eq,  .bool, .bool => some .bool
  | .ne,  .bool, .bool => some .bool
  -- unsupported combinations
  | _, _, _ => none

/-! ## Type inference for unary operators -/

/-- Infer the result type of a unary operator given operand type. -/
def inferUnOpTy (op : UnOp) (t : Ty) : Option Ty :=
  match op, t with
  | .neg, .int  => some .int
  | .not, .bool => some .bool
  | .abs, .int  => some .int
  | _, _        => none

/-! ## Main type inference function -/

/-- Infer the type of an expression under type environment `Γ`.
    Returns `some τ` if the expression is well-typed, `none` otherwise. -/
def inferType (Γ : Var → Option Ty) : Expr → Option Ty
  | .val (.int _)   => some .int
  | .val (.bool _)  => some .bool
  | .val (.float _) => some .float
  | .val (.str _)   => some .str
  | .val .none      => some .none
  | .var x          => Γ x
  | .bin op a b     =>
      match inferType Γ a, inferType Γ b with
      | some ta, some tb => inferBinOpTy op ta tb
      | _, _ => none
  | .un op a        =>
      match inferType Γ a with
      | some ta => inferUnOpTy op ta
      | none => none

/-! ## Soundness of type inference -/

/-- Soundness for value expressions (fully proved). -/
theorem inferType_sound_val (Γ : Var → Option Ty) (v : Value) (τ : Ty)
    (h : inferType Γ (.val v) = some τ) :
    HasType Γ (.val v) τ := by
  cases v with
  | int n => simp [inferType] at h; subst h; exact .intVal Γ n
  | bool b => simp [inferType] at h; subst h; exact .boolVal Γ b
  | float f => simp [inferType] at h; subst h; exact .floatVal Γ f
  | str s => simp [inferType] at h; subst h; exact .strVal Γ s
  | none => simp [inferType] at h; subst h; exact .noneVal Γ

/-- Soundness for variable expressions (fully proved). -/
theorem inferType_sound_var (Γ : Var → Option Ty) (x : Var) (τ : Ty)
    (h : inferType Γ (.var x) = some τ) :
    HasType Γ (.var x) τ := by
  simp [inferType] at h
  exact .var Γ x τ h

/-- Soundness: if `inferType Γ e = some τ`, then the typing judgment
    `HasType Γ e τ` holds. -/
theorem inferType_sound (Γ : Var → Option Ty) (e : Expr) (τ : Ty)
    (h : inferType Γ e = some τ) :
    HasType Γ e τ := by
  revert τ
  induction e with
  | val v => intro τ h; exact inferType_sound_val Γ v τ h
  | var x => intro τ h; exact inferType_sound_var Γ x τ h
  | bin op a b iha ihb =>
    intro τ h
    simp only [inferType] at h
    cases ha : inferType Γ a with
    | none => simp [ha] at h
    | some ta =>
      cases hb : inferType Γ b with
      | none => simp [ha, hb] at h
      | some tb =>
        simp [ha, hb] at h
        have iha' := iha ta ha
        have ihb' := ihb tb hb
        -- h : inferBinOpTy op ta tb = some τ
        -- Exhaust all BinOp × Ty × Ty combinations
        cases op <;> cases ta <;> cases tb <;>
          simp [inferBinOpTy] at h <;> subst h
        -- After simp+subst, only valid combinations remain
        all_goals first
          | exact .addInt Γ a b iha' ihb'
          | exact .subInt Γ a b iha' ihb'
          | exact .mulInt Γ a b iha' ihb'
          | exact .modInt Γ a b iha' ihb'
          | exact .eqInt Γ a b iha' ihb'
          | exact .neInt Γ a b iha' ihb'
          | exact .ltInt Γ a b iha' ihb'
          | exact .leInt Γ a b iha' ihb'
          | exact .gtInt Γ a b iha' ihb'
          | exact .geInt Γ a b iha' ihb'
          | exact .eqBool Γ a b iha' ihb'
          | exact .neBool Γ a b iha' ihb'
  | un op a iha =>
    intro τ h
    simp only [inferType] at h
    cases ha : inferType Γ a with
    | none => simp [ha] at h
    | some ta =>
      simp [ha] at h
      have iha' := iha ta ha
      -- h : inferUnOpTy op ta = some τ
      cases op <;> cases ta <;> simp [inferUnOpTy] at h <;> subst h
      all_goals first
        | exact .negInt Γ a iha'
        | exact .notBool Γ a iha'
        | exact .absInt Γ a iha'

/-! ## Completeness of type inference (literal + variable cases) -/

/-- Completeness for integer literals. -/
theorem inferType_complete_intVal (Γ : Var → Option Ty) (n : Int) :
    inferType Γ (.val (.int n)) = some .int := by
  simp [inferType]

/-- Completeness for boolean literals. -/
theorem inferType_complete_boolVal (Γ : Var → Option Ty) (b : Bool) :
    inferType Γ (.val (.bool b)) = some .bool := by
  simp [inferType]

/-- Completeness for float literals. -/
theorem inferType_complete_floatVal (Γ : Var → Option Ty) (f : Int) :
    inferType Γ (.val (.float f)) = some .float := by
  simp [inferType]

/-- Completeness for string literals. -/
theorem inferType_complete_strVal (Γ : Var → Option Ty) (s : String) :
    inferType Γ (.val (.str s)) = some .str := by
  simp [inferType]

/-- Completeness for None literal. -/
theorem inferType_complete_noneVal (Γ : Var → Option Ty) :
    inferType Γ (.val .none) = some .none := by
  simp [inferType]

/-- Completeness for variables. -/
theorem inferType_complete_var (Γ : Var → Option Ty) (x : Var) (τ : Ty)
    (h : Γ x = some τ) :
    inferType Γ (.var x) = some τ := by
  simp [inferType, h]

/-- Completeness for integer addition. -/
theorem inferType_complete_addInt (Γ : Var → Option Ty) (a b : Expr)
    (ha : inferType Γ a = some .int) (hb : inferType Γ b = some .int) :
    inferType Γ (.bin .add a b) = some .int := by
  simp [inferType, ha, hb, inferBinOpTy]

/-- Completeness for integer subtraction. -/
theorem inferType_complete_subInt (Γ : Var → Option Ty) (a b : Expr)
    (ha : inferType Γ a = some .int) (hb : inferType Γ b = some .int) :
    inferType Γ (.bin .sub a b) = some .int := by
  simp [inferType, ha, hb, inferBinOpTy]

/-- Completeness for integer multiplication. -/
theorem inferType_complete_mulInt (Γ : Var → Option Ty) (a b : Expr)
    (ha : inferType Γ a = some .int) (hb : inferType Γ b = some .int) :
    inferType Γ (.bin .mul a b) = some .int := by
  simp [inferType, ha, hb, inferBinOpTy]

/-- Completeness for integer negation. -/
theorem inferType_complete_negInt (Γ : Var → Option Ty) (a : Expr)
    (ha : inferType Γ a = some .int) :
    inferType Γ (.un .neg a) = some .int := by
  simp [inferType, ha, inferUnOpTy]

/-- Completeness for boolean not. -/
theorem inferType_complete_notBool (Γ : Var → Option Ty) (a : Expr)
    (ha : inferType Γ a = some .bool) :
    inferType Γ (.un .not a) = some .bool := by
  simp [inferType, ha, inferUnOpTy]

/-! ## Bool → Int type promotion for fast_int inference -/

/-- Python subtyping: bool is a subtype of int.
    This models `issubclass(bool, int) == True` in CPython and justifies
    treating bool-typed operands as int-compatible for fast_int specialization.
    Ref: 2e1cab40 perf: propagate type facts to IR fast_int/fast_float flags -/
def isIntCompatible : Ty → Bool
  | .int  => true
  | .bool => true   -- bool promotes to int in arithmetic contexts
  | _     => false

/-- Bool-to-int promotion: bool values are int-compatible. -/
theorem bool_is_int_compatible : isIntCompatible .bool = true := rfl

/-- Int values are trivially int-compatible. -/
theorem int_is_int_compatible : isIntCompatible .int = true := rfl

/-- A TypeHint permits fast_int when the known type is int-compatible. -/
def hintPermitsFastInt : TypeHint → Bool
  | .known t  => isIntCompatible t
  | .unknown  => false

/-- Unknown types do not permit fast_int (conservative). -/
theorem unknown_blocks_fast_int : hintPermitsFastInt .unknown = false := rfl

/-- If both operands of a binary expression infer to int, the expression
    permits fast_int. Together with bool_is_int_compatible, this covers
    the case where one operand is bool (promoted to int).
    Ref: 2e1cab40 perf: propagate type facts to IR fast_int/fast_float flags -/
theorem inferBinOpTy_int_int_is_int (op : BinOp) (τ : Ty)
    (h : inferBinOpTy op .int .int = some τ) :
    isIntCompatible τ = true := by
  cases op <;> simp [inferBinOpTy] at h <;> subst h <;> rfl

/-- An instruction's fast_int_hint is sound if inferType yields an
    int-compatible type for its RHS. -/
def instrFastIntSound (Γ : Var → Option Ty) (i : Instr) : Prop :=
  i.fast_int_hint = true →
    ∃ τ, inferType Γ i.rhs = some τ ∧ isIntCompatible τ = true

/-! ## Container element type propagation -/

/-- Built-in functions that always return int values.
    range() yields int elements; len(), hash(), id(), ord() return int.
    Ref: 14ad1fe3 perf: automatic int type inference from range/len/literals -/
inductive IntReturningBuiltin where
  | range_element    -- each element yielded by range() is int
  | len              -- len() always returns int
  | hash             -- hash() always returns int
  | id               -- id() always returns int
  | ord              -- ord() always returns int
  deriving DecidableEq, Repr

/-- The return type of int-returning builtins is always int. -/
def intBuiltinReturnTy : IntReturningBuiltin → Ty
  | _ => .int

/-- range() yields int elements, so `for x in range(...)` gives x : int.
    This allows setting fast_int_hint on all arithmetic involving x.
    Ref: 14ad1fe3 perf: automatic int type inference from range/len/literals -/
theorem range_yields_int :
    intBuiltinReturnTy .range_element = .int := rfl

/-- len() returns int, enabling fast_int on `i < len(xs)`. -/
theorem len_returns_int :
    intBuiltinReturnTy .len = .int := rfl

/-- hash() returns int. -/
theorem hash_returns_int :
    intBuiltinReturnTy .hash = .int := rfl

/-- Container element type propagation soundness: if the typing environment
    records a loop induction variable as int (because the iterator is range()),
    type inference correctly infers int for expressions using that variable. -/
theorem range_induction_var_permits_fast_int (Γ : Var → Option Ty) (x : Var)
    (hx : Γ x = some .int) :
    inferType Γ (.var x) = some .int := by
  simp [inferType, hx]

/-- Composition: range induction variable in an addition yields int. -/
theorem range_var_add_permits_fast_int (Γ : Var → Option Ty) (x y : Var)
    (hx : Γ x = some .int) (hy : Γ y = some .int) :
    inferType Γ (.bin .add (.var x) (.var y)) = some .int := by
  simp [inferType, hx, hy, inferBinOpTy]

/-! ## Inference + safety composition -/

/-- If type inference succeeds, the expression is type-safe (progress +
    preservation hold). This composes inferType_sound with type_safety. -/
theorem inferred_type_safe (Γ : Var → Option Ty) (ρ : Env) (e : Expr) (τ : Ty)
    (hinfer : inferType Γ e = some τ)
    (henv : envConsistent Γ ρ)
    (hmod : ∀ (a b : Expr), e = .bin .mod a b →
            ∀ va vb, evalExpr ρ a = some va → evalExpr ρ b = some vb →
            ∃ n m, va = .int n ∧ vb = .int m ∧ m ≠ 0) :
    ∃ v, evalExpr ρ e = some v ∧ valueHasTy v τ :=
  type_safety Γ ρ e τ (inferType_sound Γ e τ hinfer) henv hmod

end MoltTIR
