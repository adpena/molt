/-
  MoltTIR.TypeSystem.TypeSoundness — Progress + Preservation for MoltTIR.

  Proves type soundness for MoltTIR's expression language:
  - Progress: a well-typed expression in a consistent environment always
    evaluates to some value (evalExpr does not return none).
  - Preservation: if a well-typed expression evaluates to a value, that
    value has the expected type.
  - TypeSafety: conjunction of progress and preservation.

  The typing judgment is defined over MoltTIR.Expr (val, var, bin, un)
  and MoltTIR.Ty (int, bool, float, str, none, ...).
-/
import MoltTIR.Semantics.EvalExpr

set_option autoImplicit false

namespace MoltTIR

/-! ## Typing judgment -/

/-- Typing judgment: `HasType Γ e τ` means expression `e` has type `τ`
    under type environment `Γ`. -/
inductive HasType : (Var → Option Ty) → Expr → Ty → Prop where
  /-- Integer literal -/
  | intVal (Γ : Var → Option Ty) (n : Int) :
      HasType Γ (.val (.int n)) .int
  /-- Boolean literal -/
  | boolVal (Γ : Var → Option Ty) (b : Bool) :
      HasType Γ (.val (.bool b)) .bool
  /-- Float literal -/
  | floatVal (Γ : Var → Option Ty) (f : Int) :
      HasType Γ (.val (.float f)) .float
  /-- String literal -/
  | strVal (Γ : Var → Option Ty) (s : String) :
      HasType Γ (.val (.str s)) .str
  /-- None literal -/
  | noneVal (Γ : Var → Option Ty) :
      HasType Γ (.val .none) .none
  /-- Variable reference -/
  | var (Γ : Var → Option Ty) (x : Var) (τ : Ty) :
      Γ x = some τ → HasType Γ (.var x) τ
  /-- Integer addition -/
  | addInt (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .int → HasType Γ b .int →
      HasType Γ (.bin .add a b) .int
  /-- Integer subtraction -/
  | subInt (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .int → HasType Γ b .int →
      HasType Γ (.bin .sub a b) .int
  /-- Integer multiplication -/
  | mulInt (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .int → HasType Γ b .int →
      HasType Γ (.bin .mul a b) .int
  /-- Integer modulo (partial: divisor must be non-zero at runtime) -/
  | modInt (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .int → HasType Γ b .int →
      HasType Γ (.bin .mod a b) .int
  /-- Integer equality comparison -/
  | eqInt (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .int → HasType Γ b .int →
      HasType Γ (.bin .eq a b) .bool
  /-- Integer not-equal comparison -/
  | neInt (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .int → HasType Γ b .int →
      HasType Γ (.bin .ne a b) .bool
  /-- Integer less-than -/
  | ltInt (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .int → HasType Γ b .int →
      HasType Γ (.bin .lt a b) .bool
  /-- Integer less-or-equal -/
  | leInt (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .int → HasType Γ b .int →
      HasType Γ (.bin .le a b) .bool
  /-- Integer greater-than -/
  | gtInt (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .int → HasType Γ b .int →
      HasType Γ (.bin .gt a b) .bool
  /-- Integer greater-or-equal -/
  | geInt (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .int → HasType Γ b .int →
      HasType Γ (.bin .ge a b) .bool
  /-- Boolean equality -/
  | eqBool (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .bool → HasType Γ b .bool →
      HasType Γ (.bin .eq a b) .bool
  /-- Boolean not-equal -/
  | neBool (Γ : Var → Option Ty) (a b : Expr) :
      HasType Γ a .bool → HasType Γ b .bool →
      HasType Γ (.bin .ne a b) .bool
  /-- Integer negation -/
  | negInt (Γ : Var → Option Ty) (a : Expr) :
      HasType Γ a .int →
      HasType Γ (.un .neg a) .int
  /-- Boolean not -/
  | notBool (Γ : Var → Option Ty) (a : Expr) :
      HasType Γ a .bool →
      HasType Γ (.un .not a) .bool
  /-- Integer absolute value -/
  | absInt (Γ : Var → Option Ty) (a : Expr) :
      HasType Γ a .int →
      HasType Γ (.un .abs a) .int

/-! ## Value-type consistency -/

/-- A runtime value is consistent with a type. -/
def valueHasTy : Value → Ty → Prop
  | .int _,   .int   => True
  | .bool _,  .bool  => True
  | .float _, .float => True
  | .str _,   .str   => True
  | .none,    .none  => True
  | _,        _      => False

/-- The type of a runtime value. -/
def tyOfValue : Value → Ty
  | .int _   => .int
  | .bool _  => .bool
  | .float _ => .float
  | .str _   => .str
  | .none    => .none

theorem valueHasTy_tyOfValue (v : Value) : valueHasTy v (tyOfValue v) := by
  cases v <;> simp [valueHasTy, tyOfValue]

theorem valueHasTy_iff_eq (v : Value) (τ : Ty) :
    valueHasTy v τ ↔ tyOfValue v = τ := by
  cases v <;> cases τ <;> simp [valueHasTy, tyOfValue]

/-! ## Environment-type consistency -/

/-- A runtime environment `ρ` is consistent with a type environment `Γ` if
    every variable typed in `Γ` maps to a value of that type in `ρ`. -/
def envConsistent (Γ : Var → Option Ty) (ρ : Env) : Prop :=
  ∀ x τ, Γ x = some τ → ∃ v, ρ x = some v ∧ valueHasTy v τ

/-! ## Canonical form lemmas -/

/-- If a value has type int, it is an integer. -/
theorem canonical_int (v : Value) (h : valueHasTy v .int) :
    ∃ n, v = .int n := by
  cases v <;> simp [valueHasTy] at h
  case int n => exact ⟨n, rfl⟩

/-- If a value has type bool, it is a boolean. -/
theorem canonical_bool (v : Value) (h : valueHasTy v .bool) :
    ∃ b, v = .bool b := by
  cases v <;> simp [valueHasTy] at h
  case bool b => exact ⟨b, rfl⟩

/-- If a value has type float, it is a float. -/
theorem canonical_float (v : Value) (h : valueHasTy v .float) :
    ∃ f, v = .float f := by
  cases v <;> simp [valueHasTy] at h
  case float f => exact ⟨f, rfl⟩

/-- If a value has type str, it is a string. -/
theorem canonical_str (v : Value) (h : valueHasTy v .str) :
    ∃ s, v = .str s := by
  cases v <;> simp [valueHasTy] at h
  case str s => exact ⟨s, rfl⟩

/-- If a value has type none, it is none. -/
theorem canonical_none (v : Value) (h : valueHasTy v .none) :
    v = .none := by
  cases v <;> simp [valueHasTy] at h
  rfl

/-! ## Progress: concrete operation lemmas (fully proved) -/

/-- Progress for value expressions. -/
theorem progress_val (ρ : Env) (v : Value) :
    ∃ v', evalExpr ρ (.val v) = some v' :=
  ⟨v, rfl⟩

/-- Progress for variable expressions when binding exists. -/
theorem progress_var (ρ : Env) (x : Var) (v : Value)
    (hbind : ρ x = some v) :
    ∃ v', evalExpr ρ (.var x) = some v' :=
  ⟨v, hbind⟩

/-- Progress for integer addition given int subexpressions. -/
theorem progress_add_int (ρ : Env) (a b : Expr) (n m : Int)
    (ha : evalExpr ρ a = some (.int n))
    (hb : evalExpr ρ b = some (.int m)) :
    ∃ v, evalExpr ρ (.bin .add a b) = some v := by
  exact ⟨.int (n + m), by simp [evalExpr, ha, hb, evalBinOp]⟩

/-- Progress for integer subtraction given int subexpressions. -/
theorem progress_sub_int (ρ : Env) (a b : Expr) (n m : Int)
    (ha : evalExpr ρ a = some (.int n))
    (hb : evalExpr ρ b = some (.int m)) :
    ∃ v, evalExpr ρ (.bin .sub a b) = some v := by
  exact ⟨.int (n - m), by simp [evalExpr, ha, hb, evalBinOp]⟩

/-- Progress for integer multiplication given int subexpressions. -/
theorem progress_mul_int (ρ : Env) (a b : Expr) (n m : Int)
    (ha : evalExpr ρ a = some (.int n))
    (hb : evalExpr ρ b = some (.int m)) :
    ∃ v, evalExpr ρ (.bin .mul a b) = some v := by
  exact ⟨.int (n * m), by simp [evalExpr, ha, hb, evalBinOp]⟩

/-- Progress for integer negation. -/
theorem progress_neg_int (ρ : Env) (a : Expr) (n : Int)
    (ha : evalExpr ρ a = some (.int n)) :
    ∃ v, evalExpr ρ (.un .neg a) = some v := by
  exact ⟨.int (-n), by simp [evalExpr, ha, evalUnOp]⟩

/-- Progress for boolean not. -/
theorem progress_not_bool (ρ : Env) (a : Expr) (b : Bool)
    (ha : evalExpr ρ a = some (.bool b)) :
    ∃ v, evalExpr ρ (.un .not a) = some v := by
  exact ⟨.bool (!b), by simp [evalExpr, ha, evalUnOp]⟩

/-- Progress for integer absolute value. -/
theorem progress_abs_int (ρ : Env) (a : Expr) (n : Int)
    (ha : evalExpr ρ a = some (.int n)) :
    ∃ v, evalExpr ρ (.un .abs a) = some v := by
  exact ⟨.int (if n < 0 then -n else n), by simp [evalExpr, ha, evalUnOp]⟩

/-! ## Progress theorem (general) -/

/-- An expression is mod-safe if every mod sub-expression has a non-zero divisor. -/
def ModSafe (ρ : Env) : Expr → Prop
  | .val _ => True
  | .var _ => True
  | .bin .mod a b =>
      ModSafe ρ a ∧ ModSafe ρ b ∧
      (∀ va vb, evalExpr ρ a = some va → evalExpr ρ b = some vb →
        ∃ n m, va = .int n ∧ vb = .int m ∧ m ≠ 0)
  | .bin _ a b => ModSafe ρ a ∧ ModSafe ρ b
  | .un _ a => ModSafe ρ a

/-- Progress: a well-typed expression in a consistent environment evaluates
    to some value, provided all mod sub-expressions have non-zero divisors. -/
theorem progress (Γ : Var → Option Ty) (ρ : Env) (e : Expr) (τ : Ty)
    (htyp : HasType Γ e τ) (henv : envConsistent Γ ρ)
    (hmod : ModSafe ρ e) :
    ∃ v, evalExpr ρ e = some v := by
  induction htyp with
  | intVal => exact ⟨_, rfl⟩
  | boolVal => exact ⟨_, rfl⟩
  | floatVal => exact ⟨_, rfl⟩
  | strVal => exact ⟨_, rfl⟩
  | noneVal => exact ⟨_, rfl⟩
  | var x _ hx =>
    obtain ⟨v, hv, _⟩ := henv x _ hx
    exact ⟨v, hv⟩
  | @addInt _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .int vb (by assumption) henv hvb
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    obtain ⟨nb, rfl⟩ := canonical_int vb hvb_ty
    exact progress_add_int ρ a b na nb hva hvb
  | @subInt _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .int vb (by assumption) henv hvb
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    obtain ⟨nb, rfl⟩ := canonical_int vb hvb_ty
    exact progress_sub_int ρ a b na nb hva hvb
  | @mulInt _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .int vb (by assumption) henv hvb
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    obtain ⟨nb, rfl⟩ := canonical_int vb hvb_ty
    exact progress_mul_int ρ a b na nb hva hvb
  | @modInt _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb, hmod_nz⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .int vb (by assumption) henv hvb
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    obtain ⟨nb, rfl⟩ := canonical_int vb hvb_ty
    obtain ⟨_, _, hna, hnb, hne⟩ := hmod_nz (.int na) (.int nb) hva hvb
    simp at hna hnb; subst hna; subst hnb
    have hne' : (nb == 0) = false := by simp [beq_iff_eq]; exact hne
    exact ⟨.int (na % nb), by simp [evalExpr, hva, hvb, evalBinOp, hne']⟩
  | @eqInt _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .int vb (by assumption) henv hvb
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    obtain ⟨nb, rfl⟩ := canonical_int vb hvb_ty
    exact ⟨.bool (na == nb), by simp [evalExpr, hva, hvb, evalBinOp]⟩
  | @neInt _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .int vb (by assumption) henv hvb
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    obtain ⟨nb, rfl⟩ := canonical_int vb hvb_ty
    exact ⟨.bool (na != nb), by simp [evalExpr, hva, hvb, evalBinOp]⟩
  | @ltInt _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .int vb (by assumption) henv hvb
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    obtain ⟨nb, rfl⟩ := canonical_int vb hvb_ty
    exact ⟨.bool (decide (na < nb)), by simp [evalExpr, hva, hvb, evalBinOp]⟩
  | @leInt _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .int vb (by assumption) henv hvb
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    obtain ⟨nb, rfl⟩ := canonical_int vb hvb_ty
    exact ⟨.bool (decide (na ≤ nb)), by simp [evalExpr, hva, hvb, evalBinOp]⟩
  | @gtInt _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .int vb (by assumption) henv hvb
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    obtain ⟨nb, rfl⟩ := canonical_int vb hvb_ty
    exact ⟨.bool (decide (na > nb)), by simp [evalExpr, hva, hvb, evalBinOp]⟩
  | @geInt _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .int vb (by assumption) henv hvb
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    obtain ⟨nb, rfl⟩ := canonical_int vb hvb_ty
    exact ⟨.bool (decide (na ≥ nb)), by simp [evalExpr, hva, hvb, evalBinOp]⟩
  | @eqBool _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .bool va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .bool vb (by assumption) henv hvb
    obtain ⟨ba, rfl⟩ := canonical_bool va hva_ty
    obtain ⟨bb, rfl⟩ := canonical_bool vb hvb_ty
    exact ⟨.bool (ba == bb), by simp [evalExpr, hva, hvb, evalBinOp]⟩
  | @neBool _ a b _ _ iha ihb =>
    obtain ⟨hma, hmb⟩ := hmod
    obtain ⟨va, hva⟩ := iha henv hma
    obtain ⟨vb, hvb⟩ := ihb henv hmb
    have hva_ty := preservation Γ ρ a .bool va (by assumption) henv hva
    have hvb_ty := preservation Γ ρ b .bool vb (by assumption) henv hvb
    obtain ⟨ba, rfl⟩ := canonical_bool va hva_ty
    obtain ⟨bb, rfl⟩ := canonical_bool vb hvb_ty
    exact ⟨.bool (ba != bb), by simp [evalExpr, hva, hvb, evalBinOp]⟩
  | @negInt _ a _ iha =>
    obtain ⟨va, hva⟩ := iha henv hmod
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    exact progress_neg_int ρ a na hva
  | @notBool _ a _ iha =>
    obtain ⟨va, hva⟩ := iha henv hmod
    have hva_ty := preservation Γ ρ a .bool va (by assumption) henv hva
    obtain ⟨ba, rfl⟩ := canonical_bool va hva_ty
    exact progress_not_bool ρ a ba hva
  | @absInt _ a _ iha =>
    obtain ⟨va, hva⟩ := iha henv hmod
    have hva_ty := preservation Γ ρ a .int va (by assumption) henv hva
    obtain ⟨na, rfl⟩ := canonical_int va hva_ty
    exact progress_abs_int ρ a na hva

/-! ## Preservation theorem -/

/-- Preservation: if a well-typed expression evaluates to a value, that
    value has the expected type. -/
theorem preservation (Γ : Var → Option Ty) (ρ : Env) (e : Expr) (τ : Ty) (v : Value)
    (htyp : HasType Γ e τ) (henv : envConsistent Γ ρ)
    (heval : evalExpr ρ e = some v) :
    valueHasTy v τ := by
  induction htyp with
  | intVal =>
    simp [evalExpr] at heval; subst heval; trivial
  | boolVal =>
    simp [evalExpr] at heval; subst heval; trivial
  | floatVal =>
    simp [evalExpr] at heval; subst heval; trivial
  | strVal =>
    simp [evalExpr] at heval; subst heval; trivial
  | noneVal =>
    simp [evalExpr] at heval; subst heval; trivial
  | var x _ hx =>
    obtain ⟨v', hv', hvt⟩ := henv x _ hx
    simp [evalExpr] at heval
    rw [hv'] at heval; simp at heval; subst heval; exact hvt
  | @addInt _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      obtain ⟨nb, rfl⟩ := canonical_int vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @subInt _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      obtain ⟨nb, rfl⟩ := canonical_int vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @mulInt _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      obtain ⟨nb, rfl⟩ := canonical_int vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @modInt _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      obtain ⟨nb, rfl⟩ := canonical_int vb hvb
      simp [evalBinOp] at heval
      split at heval
      · contradiction
      · simp at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @eqInt _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      obtain ⟨nb, rfl⟩ := canonical_int vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @neInt _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      obtain ⟨nb, rfl⟩ := canonical_int vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @ltInt _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      obtain ⟨nb, rfl⟩ := canonical_int vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @leInt _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      obtain ⟨nb, rfl⟩ := canonical_int vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @gtInt _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      obtain ⟨nb, rfl⟩ := canonical_int vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @geInt _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      obtain ⟨nb, rfl⟩ := canonical_int vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @eqBool _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨ba, rfl⟩ := canonical_bool va hva
      obtain ⟨bb, rfl⟩ := canonical_bool vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @neBool _ a b ha_ty hb_ty iha ihb =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a, hb_eval : evalExpr ρ b with
    | some va, some vb =>
      simp [ha_eval, hb_eval] at heval
      have hva := iha ha_eval
      have hvb := ihb hb_eval
      obtain ⟨ba, rfl⟩ := canonical_bool va hva
      obtain ⟨bb, rfl⟩ := canonical_bool vb hvb
      simp [evalBinOp] at heval; subst heval; trivial
    | some _, none => simp [ha_eval, hb_eval] at heval
    | none, _ => simp [ha_eval] at heval
  | @negInt _ a ha_ty iha =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a with
    | some va =>
      simp [ha_eval] at heval
      have hva := iha ha_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      simp [evalUnOp] at heval; subst heval; trivial
    | none => simp [ha_eval] at heval
  | @notBool _ a ha_ty iha =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a with
    | some va =>
      simp [ha_eval] at heval
      have hva := iha ha_eval
      obtain ⟨ba, rfl⟩ := canonical_bool va hva
      simp [evalUnOp] at heval; subst heval; trivial
    | none => simp [ha_eval] at heval
  | @absInt _ a ha_ty iha =>
    simp only [evalExpr] at heval
    match ha_eval : evalExpr ρ a with
    | some va =>
      simp [ha_eval] at heval
      have hva := iha ha_eval
      obtain ⟨na, rfl⟩ := canonical_int va hva
      simp [evalUnOp] at heval; subst heval; trivial
    | none => simp [ha_eval] at heval

/-! ## Preservation: concrete operation lemmas (fully proved) -/

/-- Preservation for value expressions. -/
theorem preservation_val (v v' : Value) (τ : Ty) (ρ : Env)
    (heval : evalExpr ρ (.val v) = some v')
    (hvt : valueHasTy v τ) :
    valueHasTy v' τ := by
  simp [evalExpr] at heval; subst heval; exact hvt

/-- Preservation for integer addition. -/
theorem preservation_add_int (ρ : Env) (a b : Expr) (v : Value)
    (n m : Int)
    (ha : evalExpr ρ a = some (.int n))
    (hb : evalExpr ρ b = some (.int m))
    (heval : evalExpr ρ (.bin .add a b) = some v) :
    valueHasTy v .int := by
  simp [evalExpr, ha, hb, evalBinOp] at heval; subst heval; trivial

/-- Preservation for integer subtraction. -/
theorem preservation_sub_int (ρ : Env) (a b : Expr) (v : Value)
    (n m : Int)
    (ha : evalExpr ρ a = some (.int n))
    (hb : evalExpr ρ b = some (.int m))
    (heval : evalExpr ρ (.bin .sub a b) = some v) :
    valueHasTy v .int := by
  simp [evalExpr, ha, hb, evalBinOp] at heval; subst heval; trivial

/-- Preservation for integer negation. -/
theorem preservation_neg_int (ρ : Env) (a : Expr) (v : Value)
    (n : Int)
    (ha : evalExpr ρ a = some (.int n))
    (heval : evalExpr ρ (.un .neg a) = some v) :
    valueHasTy v .int := by
  simp [evalExpr, ha, evalUnOp] at heval; subst heval; trivial

/-- Preservation for boolean not. -/
theorem preservation_not_bool (ρ : Env) (a : Expr) (v : Value)
    (b : Bool)
    (ha : evalExpr ρ a = some (.bool b))
    (heval : evalExpr ρ (.un .not a) = some v) :
    valueHasTy v .bool := by
  simp [evalExpr, ha, evalUnOp] at heval; subst heval; trivial

/-! ## Type safety -/

/-- Type safety: conjunction of progress and preservation.
    A well-typed expression in a consistent environment either evaluates to
    a value of the expected type, provided all mod sub-expressions have
    non-zero divisors. -/
theorem type_safety (Γ : Var → Option Ty) (ρ : Env) (e : Expr) (τ : Ty)
    (htyp : HasType Γ e τ) (henv : envConsistent Γ ρ)
    (hmod : ModSafe ρ e) :
    ∃ v, evalExpr ρ e = some v ∧ valueHasTy v τ := by
  have ⟨v, hv⟩ := progress Γ ρ e τ htyp henv hmod
  exact ⟨v, hv, preservation Γ ρ e τ v htyp henv hv⟩

end MoltTIR
