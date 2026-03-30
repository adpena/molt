/-
  Inversion lemmas for lowerExpr.
-/
import MoltLowering.ASTtoTIR

set_option autoImplicit false

namespace MoltLowering

private theorem lowerExpr_binOp_eq (nm : NameMap) (op : MoltPython.BinOp)
    (l r : MoltPython.PyExpr) :
    lowerExpr nm (.binOp op l r) =
    match lowerExpr nm l, lowerExpr nm r with
    | some la, some ra => some (.bin (lowerBinOp op) la ra)
    | _, _ => none := by rfl

/-- Inversion for lowerExpr on binOp. -/
theorem lowerExpr_binOp_inv (nm : NameMap) (op : MoltPython.BinOp)
    (left right : MoltPython.PyExpr) (te : MoltTIR.Expr)
    (h : lowerExpr nm (.binOp op left right) = some te) :
    ∃ la ra, lowerExpr nm left = some la ∧ lowerExpr nm right = some ra ∧
      te = .bin (lowerBinOp op) la ra := by
  rw [lowerExpr_binOp_eq] at h
  match hl : lowerExpr nm left, hr : lowerExpr nm right with
  | some la, some ra =>
    simp [hl, hr] at h; exact ⟨la, ra, rfl, rfl, h.symm⟩
  | some _, none => simp [hl, hr] at h
  | none, some _ => simp [hl] at h
  | none, none => simp [hl] at h

private theorem lowerExpr_unaryOp_eq (nm : NameMap) (op : MoltPython.UnaryOp)
    (operand : MoltPython.PyExpr) :
    lowerExpr nm (.unaryOp op operand) =
    match lowerExpr nm operand with
    | some a => some (.un (lowerUnaryOp op) a)
    | none => none := by rfl

/-- Inversion for lowerExpr on unaryOp. -/
theorem lowerExpr_unaryOp_inv (nm : NameMap) (op : MoltPython.UnaryOp)
    (operand : MoltPython.PyExpr) (te : MoltTIR.Expr)
    (h : lowerExpr nm (.unaryOp op operand) = some te) :
    ∃ a, lowerExpr nm operand = some a ∧ te = .un (lowerUnaryOp op) a := by
  rw [lowerExpr_unaryOp_eq] at h
  match ho : lowerExpr nm operand with
  | some a => simp [ho] at h; exact ⟨a, rfl, h.symm⟩
  | none => simp [ho] at h

end MoltLowering
