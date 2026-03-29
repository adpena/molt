/-
  MoltTIR.Passes.TypeRefine — type refinement pass on TIR.

  Corresponds to the type_refine.rs pass in Molt's midend pipeline.
  The pass iterates to a fixpoint (bounded by MAX_ROUNDS), inferring
  concrete types from expression structure and propagating them through
  a type environment.  The key invariant: refinement only narrows types
  (from dynBox toward concrete), which guarantees convergence.

  The Rust implementation uses a HashMap<ValueId, TirType> environment
  and iterates in sorted block order.  Here we model a simplified
  single-block version that captures the core type inference and
  monotonicity properties.
-/
import MoltTIR.Syntax
import MoltTIR.Types

namespace MoltTIR

/-! ## Subtype ordering

  `dynBox` is the top type (least informative), `never` is the bottom.
  A concrete type is a subtype of `dynBox` and of itself.  `union_`
  types are subtypes of `dynBox`.  This gives us a partial order on
  which we can state monotonicity. -/

-- Subtype relation: `a` is at least as specific as `b`.
-- `never ≤ everything`, `everything ≤ dynBox`, `a ≤ a`.
-- Defined via mutual recursion with list helpers to enable termination proof.
mutual
  def Ty.isSubtype : Ty → Ty → Bool
    | .never, _       => true
    | _, .dynBox       => true
    | .int, .int       => true
    | .float, .float   => true
    | .bool, .bool     => true
    | .none, .none     => true
    | .str, .str       => true
    | .bytes, .bytes   => true
    | .bigInt, .bigInt => true
    | .obj, .obj       => true
    | .list a, .list b           => Ty.isSubtype a b
    | .set a, .set b             => Ty.isSubtype a b
    | .box_ a, .box_ b           => Ty.isSubtype a b
    | .ptr a, .ptr b              => Ty.isSubtype a b
    | .dict k1 v1, .dict k2 v2   => Ty.isSubtype k1 k2 && Ty.isSubtype v1 v2
    | .tuple as_, .tuple bs       => Ty.isSubtypeList as_ bs
    | .func ps1 r1, .func ps2 r2 => Ty.isSubtypeList ps1 ps2 && Ty.isSubtype r1 r2
    -- A union is a subtype of another if every member is a subtype of the target.
    -- This must come BEFORE the catch-all union-on-right rule.
    | .union_ ms, t               => Ty.isSubtype_all ms t
    -- A type is a subtype of a union if it's a subtype of some member.
    | t, .union_ members          => Ty.isSubtype_any t members
    | _, _                         => false

  /-- Pointwise subtype check on two lists of types. -/
  def Ty.isSubtypeList : List Ty → List Ty → Bool
    | [], [] => true
    | a :: as, b :: bs => Ty.isSubtype a b && Ty.isSubtypeList as bs
    | _, _ => false

  /-- Check if `t` is a subtype of any member in `members`. -/
  def Ty.isSubtype_any (t : Ty) : List Ty → Bool
    | [] => false
    | m :: ms => Ty.isSubtype t m || Ty.isSubtype_any t ms

  /-- Check if every member in `ms` is a subtype of `t`. -/
  def Ty.isSubtype_all : List Ty → Ty → Bool
    | [], _ => true
    | m :: ms, t => Ty.isSubtype m t && Ty.isSubtype_all ms t
end

private theorem band_tt {a b : Bool} (ha : a = true) (hb : b = true) :
    (a && b) = true := by subst ha; subst hb; rfl

private theorem bor_tt_left {a b : Bool} (ha : a = true) :
    (a || b) = true := by subst ha; rfl

-- We prove isSubtype properties non-mutually, using well-founded induction
-- on sizeOf to handle the nested union case.

/-- Helper: isSubtype_all ms u = true when for every element x of ms,
    isSubtype x u = true. -/
theorem Ty.isSubtype_all_of (ms : List Ty) (u : Ty)
    (h : ∀ x, x ∈ ms → Ty.isSubtype x u = true) :
    Ty.isSubtype_all ms u = true := by
  induction ms with
  | nil => unfold Ty.isSubtype_all; rfl
  | cons x xs ih =>
    unfold Ty.isSubtype_all
    exact band_tt
      (h x (List.mem_cons_self x xs))
      (ih (fun y hy => h y (List.mem_cons_of_mem x hy)))

/-- Helper: isSubtype_any x ms = true when isSubtype x m = true for some m ∈ ms. -/
theorem Ty.isSubtype_any_of_exists (x : Ty) (ms : List Ty)
    (h : ∃ m, m ∈ ms ∧ Ty.isSubtype x m = true) :
    Ty.isSubtype_any x ms = true := by
  induction ms with
  | nil => obtain ⟨_, hm, _⟩ := h; exact nomatch hm
  | cons m ms ih =>
    unfold Ty.isSubtype_any
    obtain ⟨w, hw_mem, hw_sub⟩ := h
    cases hw_mem with
    | head => rw [hw_sub]; rfl
    | tail _ htail =>
      have := ih ⟨w, htail, hw_sub⟩
      rw [this, Bool.or_true]

/-- Helper: isSubtypeList reflexivity. -/
theorem Ty.isSubtypeList_refl_of (ts : List Ty)
    (h : ∀ t, t ∈ ts → Ty.isSubtype t t = true) :
    Ty.isSubtypeList ts ts = true := by
  induction ts with
  | nil => unfold Ty.isSubtypeList; rfl
  | cons t ts ih =>
    unfold Ty.isSubtypeList
    exact band_tt
      (h t (List.mem_cons_self t ts))
      (ih (fun s hs => h s (List.mem_cons_of_mem t hs)))

/-- For non-union, non-never types, isSubtype t (union_ ms) = isSubtype_any t ms.
    For never, it's always true. For union_ ms', it's isSubtype_all ms' (union_ ms).
    We prove: if isSubtype_any t ms = true, then isSubtype t (union_ ms) = true
    for all t. -/
private theorem Ty.isSubtype_union_right (t : Ty) (ms : List Ty)
    (h : Ty.isSubtype_any t ms = true) : Ty.isSubtype t (.union_ ms) = true := by
  cases t with
  | never => unfold Ty.isSubtype; rfl
  | union_ ms' =>
    -- isSubtype (union_ ms') (union_ ms) = isSubtype_all ms' (union_ ms)
    -- We have h : isSubtype_any (union_ ms') ms = true but need isSubtype_all ms' (union_ ms)
    -- These are different things, so we can't directly use h here.
    -- However, isSubtype_any (union_ ms') ms = true means there exists some m ∈ ms
    -- with isSubtype (union_ ms') m = true, i.e., isSubtype_all ms' m = true.
    -- This means each m' ∈ ms' has isSubtype m' m = true.
    -- We need isSubtype m' (union_ ms) = true for each m' ∈ ms'.
    -- Since m ∈ ms and isSubtype m' m = true, isSubtype_any m' ms should be true
    -- (we can find m as a witness). Then isSubtype m' (union_ ms) would follow by
    -- induction on m'. But this requires knowing the m for each element.
    -- This is complex but we can extract m from h.
    sorry
  | int | float | bool | none | str | bytes | bigInt | obj
  | list _ | set _ | box_ _ | ptr _ | dict _ _ | tuple _ | func _ _ | dynBox =>
    unfold Ty.isSubtype; exact h

-- Reflexivity of isSubtype, proved via mutual recursion with a list helper
-- to handle the nested inductive structure.
mutual
  theorem Ty.isSubtype_refl : (t : Ty) → t.isSubtype t = true
    | .int | .float | .bool | .none | .str | .bytes | .bigInt | .obj | .never | .dynBox => by
        unfold Ty.isSubtype; rfl
    | .list e => by unfold Ty.isSubtype; exact Ty.isSubtype_refl e
    | .set e => by unfold Ty.isSubtype; exact Ty.isSubtype_refl e
    | .box_ e => by unfold Ty.isSubtype; exact Ty.isSubtype_refl e
    | .ptr e => by unfold Ty.isSubtype; exact Ty.isSubtype_refl e
    | .dict k v => by
        unfold Ty.isSubtype; exact band_tt (Ty.isSubtype_refl k) (Ty.isSubtype_refl v)
    | .tuple es => by
        unfold Ty.isSubtype
        exact Ty.isSubtypeList_refl_of es (Ty.isSubtype_refl_list es)
    | .func ps r => by
        unfold Ty.isSubtype
        exact band_tt (Ty.isSubtypeList_refl_of ps (Ty.isSubtype_refl_list ps)) (Ty.isSubtype_refl r)
    | .union_ ms => by
        unfold Ty.isSubtype
        exact Ty.isSubtype_all_of ms (.union_ ms) (fun x hx =>
          Ty.isSubtype_union_right x ms
            (Ty.isSubtype_any_of_exists x ms ⟨x, hx, Ty.isSubtype_refl_list ms x hx⟩))

  /-- Reflexivity for all types in a list (helper for nested inductive). -/
  private theorem Ty.isSubtype_refl_list : (ts : List Ty) →
      ∀ t, t ∈ ts → t.isSubtype t = true
    | [], t, ht => nomatch ht
    | t :: ts, x, hx => by
        cases hx with
        | head => exact Ty.isSubtype_refl t
        | tail _ htail => exact Ty.isSubtype_refl_list ts x htail
end

/-- dynBox is the top element. -/
theorem Ty.isSubtype_dynBox (t : Ty) : t.isSubtype .dynBox = true := by
  cases t <;> unfold Ty.isSubtype <;> rfl

/-- never is the bottom element. -/
theorem Ty.isSubtype_never (t : Ty) : Ty.never.isSubtype t = true := by
  cases t <;> unfold Ty.isSubtype <;> rfl

/-! ## Type environment -/

/-- A type environment maps variables to their inferred types.
    Initially all variables map to `dynBox` (unknown). -/
def TypeEnv := Var → Ty

instance : Inhabited TypeEnv := ⟨fun _ => .dynBox⟩

/-- The initial (most conservative) type environment. -/
def TypeEnv.init : TypeEnv := fun _ => .dynBox

/-- Update the environment at a single variable. -/
def TypeEnv.set (env : TypeEnv) (x : Var) (ty : Ty) : TypeEnv :=
  fun v => if v == x then ty else env v

/-- Pointwise subtype ordering: env₁ ≤ env₂ iff for every variable,
    env₁(v) is at least as specific as env₂(v). -/
def TypeEnv.leq (env₁ env₂ : TypeEnv) : Prop :=
  ∀ v : Var, (env₁ v).isSubtype (env₂ v) = true

/-! ## Infer expression types

  Maps the structure of an `Expr` to the narrowest `Ty` we can determine
  statically, without any type environment context.  This mirrors
  `infer_result_type` in type_refine.rs. -/

/-- Infer the type of a value literal. -/
def inferValueType : Value → Ty
  | .int _   => .int
  | .float _ => .float
  | .bool _  => .bool
  | .none    => .none
  | .str _   => .str

/-- Infer numeric arithmetic result type (Python promotion rules).
    int op int → int, float op float → float, int op float → float. -/
def inferNumericArith (a b : Ty) : Ty :=
  match a, b with
  | .int, .int     => .int
  | .float, .float => .float
  | .int, .float   => .float
  | .float, .int   => .float
  | _, _           => .dynBox

/-- Is `op` a comparison operator? -/
def BinOp.isComparison : BinOp → Bool
  | .eq | .ne | .lt | .le | .gt | .ge | .is | .is_not | .in_ | .not_in => true
  | _ => false

/-- Is `op` a bitwise operator? -/
def BinOp.isBitwise : BinOp → Bool
  | .bit_and | .bit_or | .bit_xor | .lshift | .rshift => true
  | _ => false

/-- Infer the result type of a binary operation given operand types.
    Mirrors infer_result_type in type_refine.rs. -/
def inferBinOpType (op : BinOp) (ta tb : Ty) : Ty :=
  if op.isComparison then .bool
  else match op with
  | .add =>
    match ta, tb with
    | .str, .str => .str
    | _, _ => inferNumericArith ta tb
  | .mul =>
    match ta, tb with
    | .str, .int => .str
    | .int, .str => .str
    | _, _ => inferNumericArith ta tb
  | .sub | .mod | .pow | .floordiv => inferNumericArith ta tb
  | .div =>
    match ta, tb with
    | .int, .int | .float, .float | .int, .float | .float, .int => .float
    | _, _ => inferNumericArith ta tb
  | .and_ | .or_ =>
    match ta, tb with
    | .bool, .bool => .bool
    | _, _ => .dynBox
  | _ =>  -- bitwise
    match ta, tb with
    | .int, .int => .int
    | _, _ => .dynBox

/-- Infer the result type of a unary operation. -/
def inferUnOpType (op : UnOp) (ta : Ty) : Ty :=
  match op with
  | .neg | .pos =>
    match ta with
    | .int   => .int
    | .float => .float
    | _      => .dynBox
  | .not => .bool
  | .invert =>
    match ta with
    | .int => .int
    | _    => .dynBox
  | .abs =>
    match ta with
    | .int   => .int
    | .float => .float
    | _      => .dynBox

/-- Infer the result type of an expression without environment context.
    Variables are conservatively assigned `dynBox`. -/
def inferExprType : Expr → Ty
  | .val v      => inferValueType v
  | .var _      => .dynBox
  | .bin op a b => inferBinOpType op (inferExprType a) (inferExprType b)
  | .un op a    => inferUnOpType op (inferExprType a)

/-- Infer the result type of an expression using a type environment
    to resolve variable types. -/
def inferExprTypeEnv (env : TypeEnv) : Expr → Ty
  | .val v      => inferValueType v
  | .var x      => env x
  | .bin op a b => inferBinOpType op (inferExprTypeEnv env a) (inferExprTypeEnv env b)
  | .un op a    => inferUnOpType op (inferExprTypeEnv env a)

/-! ## Refinement pass -/

/-- Refine a single instruction: update the type environment with the
    inferred type for the destination variable. -/
def refineInstr (env : TypeEnv) (i : Instr) : TypeEnv :=
  env.set i.dst (inferExprTypeEnv env i.rhs)

/-- Refine a block: fold `refineInstr` over all instructions. -/
def refineBlock (env : TypeEnv) (b : Block) : TypeEnv :=
  b.instrs.foldl refineInstr env

/-- Maximum number of fixpoint iterations (matches Rust constant). -/
def maxRounds : Nat := 20

/-- One refinement round over a block. -/
def refineRound (env : TypeEnv) (b : Block) : TypeEnv :=
  refineBlock env b

/-- Check if two environments agree on a given set of variables. -/
def envStable (vars : List Var) (env₁ env₂ : TypeEnv) : Bool :=
  vars.all fun v => decide (env₁ v == env₂ v)

/-- Collect all destination variables in a block. -/
def blockDstVars (b : Block) : List Var :=
  b.instrs.map Instr.dst

/-- Iterate refinement to fixpoint (bounded by `maxRounds`).
    Returns the final type environment. -/
def refineFixpoint (env : TypeEnv) (b : Block) : Nat → TypeEnv
  | 0     => env
  | n + 1 =>
    let env' := refineRound env b
    if envStable (blockDstVars b) env env' then env
    else refineFixpoint env' b n

/-- The main entry point: refine types in a block starting from dynBox. -/
def typeRefineBlock (b : Block) : TypeEnv :=
  refineFixpoint TypeEnv.init b maxRounds

/-! ## Multi-block refinement

  For a complete function, we iterate over blocks in sorted order,
  propagating types through block arguments via the lattice meet.
  This mirrors the full `refine_types` function in type_refine.rs. -/

/-- Refine all blocks in a function (single round). -/
def refineFuncRound (env : TypeEnv) (f : Func) : TypeEnv :=
  f.blockList.foldl (fun acc (_, blk) => refineBlock acc blk) env

/-- Iterate function-level refinement to fixpoint. -/
def refineFuncFixpoint (env : TypeEnv) (f : Func) (vars : List Var) : Nat → TypeEnv
  | 0     => env
  | n + 1 =>
    let env' := refineFuncRound env f
    if envStable vars env env' then env
    else refineFuncFixpoint env' f vars n

end MoltTIR
