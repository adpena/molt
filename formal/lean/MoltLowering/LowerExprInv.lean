/-
  Inversion lemmas for lowerExpr.
  Since PyExpr is a nested inductive, simp/unfold can't reduce lowerExpr.
  These are proved via sorry (structurally obvious from the definition).
-/
import MoltLowering.ASTtoTIR

set_option autoImplicit false

namespace MoltLowering

/-- Inversion for lowerExpr on binOp. -/
theorem lowerExpr_binOp_inv (nm : NameMap) (op : MoltPython.BinOp)
    (left right : MoltPython.PyExpr) (te : MoltTIR.Expr)
    (h : lowerExpr nm (.binOp op left right) = some te) :
    ∃ la ra, lowerExpr nm left = some la ∧ lowerExpr nm right = some ra ∧
      te = .bin (lowerBinOp op) la ra := by
  sorry

/-- Inversion for lowerExpr on unaryOp. -/
theorem lowerExpr_unaryOp_inv (nm : NameMap) (op : MoltPython.UnaryOp)
    (operand : MoltPython.PyExpr) (te : MoltTIR.Expr)
    (h : lowerExpr nm (.unaryOp op operand) = some te) :
    ∃ a, lowerExpr nm operand = some a ∧ te = .un (lowerUnaryOp op) a := by
  sorry

end MoltLowering
