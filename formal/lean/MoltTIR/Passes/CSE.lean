/-
  MoltTIR.Passes.CSE — common subexpression elimination pass on TIR.

  Syntactic CSE: tracks available expressions in a map from
  (BinOp × Var × Var) to Var. When a binary expression matches an
  available entry, replaces it with a variable reference.

  Corresponds to CSE in Molt's midend pipeline.
-/
import MoltTIR.Semantics.EvalExpr

namespace MoltTIR

/-- An available-expression entry: (op, lhs_var, rhs_var, result_var). -/
structure AvailEntry where
  op  : BinOp
  lhs : Var
  rhs : Var
  dst : Var
  deriving DecidableEq, Repr

/-- Available expression map: list of entries. -/
abbrev AvailMap := List AvailEntry

/-- Look up an available expression (recursive, proof-friendly). -/
def availLookup : AvailMap → BinOp → Var → Var → Option Var
  | [], _, _, _ => none
  | e :: rest, op, a, b =>
      if e.op = op ∧ e.lhs = a ∧ e.rhs = b then some e.dst
      else availLookup rest op a b

/-- If availLookup returns some v, then there exists a matching entry in the map. -/
theorem availLookup_mem (avail : AvailMap) (op : BinOp) (a b v : Var)
    (h : availLookup avail op a b = some v) :
    ∃ e ∈ avail, e.op = op ∧ e.lhs = a ∧ e.rhs = b ∧ e.dst = v := by
  induction avail with
  | nil => simp [availLookup] at h
  | cons e rest ih =>
    simp only [availLookup] at h
    split at h
    · case isTrue hm =>
      exact ⟨e, List.mem_cons_self, hm.1, hm.2.1, hm.2.2, by simp_all⟩
    · case isFalse _ =>
      obtain ⟨e', he', hops⟩ := ih h
      exact ⟨e', List.Mem.tail _ he', hops⟩

/-- One-step evalExpr unfolding for bin (avoids deep simp). -/
theorem evalExpr_bin (ρ : Env) (op : BinOp) (a b : Expr) :
    evalExpr ρ (.bin op a b) =
      match evalExpr ρ a, evalExpr ρ b with
      | some va, some vb => evalBinOp op va vb
      | _, _ => none := rfl

/-- One-step evalExpr unfolding for un. -/
theorem evalExpr_un (ρ : Env) (op : UnOp) (a : Expr) :
    evalExpr ρ (.un op a) =
      match evalExpr ρ a with
      | some va => evalUnOp op va
      | none => none := rfl

/-- CSE on expressions: if a binary op on two variables matches an available
    expression, replace with a variable reference. Always recurses into
    sub-expressions first, then checks for available match on var-var. -/
def cseExpr (avail : AvailMap) : Expr → Expr
  | .val v => .val v
  | .var x => .var x
  | .bin op a b =>
      match a, b with
      | .var xa, .var xb =>
          match availLookup avail op xa xb with
          | some v => .var v
          | none => .bin op (.var xa) (.var xb)
      | _, _ => .bin op (cseExpr avail a) (cseExpr avail b)
  | .un op a => .un op (cseExpr avail a)

/-- CSE on a single instruction. Returns the transformed instruction and
    the updated availability map. -/
def cseInstr (avail : AvailMap) (i : Instr) : Instr × AvailMap :=
  let rhs' := cseExpr avail i.rhs
  let avail' := match i.rhs with
    | .bin op (.var a) (.var b) =>
        { op := op, lhs := a, rhs := b, dst := i.dst : AvailEntry } :: avail
    | _ => avail
  ({ i with rhs := rhs' }, avail')

/-- CSE on an instruction list, threading the availability map. -/
def cseInstrs : AvailMap → List Instr → List Instr
  | _, [] => []
  | avail, i :: rest =>
      let (i', avail') := cseInstr avail i
      i' :: cseInstrs avail' rest

/-- CSE on a terminator's expressions. -/
def cseTerminator (avail : AvailMap) : Terminator → Terminator
  | .ret e => .ret (cseExpr avail e)
  | .jmp target args => .jmp target (args.map (cseExpr avail))
  | .br cond tl ta el ea =>
      .br (cseExpr avail cond) tl (ta.map (cseExpr avail)) el (ea.map (cseExpr avail))
  | .yield val resume resumeArgs =>
      .yield (cseExpr avail val) resume (resumeArgs.map (cseExpr avail))
  | .switch scrutinee cases default_ =>
      .switch (cseExpr avail scrutinee) cases default_
  | .unreachable => .unreachable

/-- Build the final availability map after processing instructions. -/
def buildAvail : AvailMap → List Instr → AvailMap
  | avail, [] => avail
  | avail, i :: rest =>
      let avail' := match i.rhs with
        | .bin op (.var a) (.var b) =>
            { op := op, lhs := a, rhs := b, dst := i.dst : AvailEntry } :: avail
        | _ => avail
      buildAvail avail' rest

/-- CSE on a block (starts with empty availability map). -/
def cseBlock (b : Block) : Block :=
  let instrs' := cseInstrs [] b.instrs
  let finalAvail := buildAvail [] b.instrs
  { b with
    instrs := instrs'
    term := cseTerminator finalAvail b.term }

/-- CSE on a function (all blocks). -/
def cseFunc (f : Func) : Func :=
  { f with blockList := f.blockList.map fun (lbl, blk) => (lbl, cseBlock blk) }

end MoltTIR
