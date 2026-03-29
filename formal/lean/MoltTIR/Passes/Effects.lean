/-
  MoltTIR.Passes.Effects — effect classification for TIR operations.

  In the current model all expressions are pure (no heap, no I/O).
  This module provides the vocabulary and lattice structure so that
  future extensions (heap reads/writes, control effects) slot in
  without restructuring the proof infrastructure.
-/
import MoltTIR.Syntax

namespace MoltTIR

/-- Effect classification, ordered by increasing observability.
    pure ≤ reads ≤ writes ≤ control. -/
inductive Effect where
  | pure     -- no observable side effects
  | reads    -- reads heap / global state
  | writes   -- writes heap / global state
  | control  -- non-local control flow (exceptions, divergence)
  deriving DecidableEq, Repr

namespace Effect

/-- Numeric rank for ordering. -/
def rank : Effect → Nat
  | .pure    => 0
  | .reads   => 1
  | .writes  => 2
  | .control => 3

instance : LE Effect where
  le a b := a.rank ≤ b.rank

instance : LT Effect where
  lt a b := a.rank < b.rank

instance (a b : Effect) : Decidable (a ≤ b) :=
  inferInstanceAs (Decidable (a.rank ≤ b.rank))

instance (a b : Effect) : Decidable (a < b) :=
  inferInstanceAs (Decidable (a.rank < b.rank))

/-- Join (least upper bound) of two effects. -/
def join (a b : Effect) : Effect :=
  if b ≤ a then a else b

theorem le_refl (a : Effect) : a ≤ a := Nat.le_refl _

theorem le_trans {a b c : Effect} (h1 : a ≤ b) (h2 : b ≤ c) : a ≤ c :=
  Nat.le_trans h1 h2

theorem join_comm (a b : Effect) : join a b = join b a := by
  cases a <;> cases b <;> rfl

theorem join_idem (a : Effect) : join a a = a := by
  cases a <;> rfl

end Effect

/-- Effect of an expression. In the current pure model, always .pure. -/
def exprEffect : Expr → Effect
  | .val _ => .pure
  | .var _ => .pure
  | .bin _ a b => Effect.join (exprEffect a) (exprEffect b)
  | .un _ a => exprEffect a

/-- Effect of an instruction. -/
def instrEffect (i : Instr) : Effect := exprEffect i.rhs

/-- Effect of a terminator. Returns are pure; jumps and branches are control. -/
def termEffect : Terminator → Effect
  | .ret _ => .pure
  | .jmp _ _ => .control
  | .br _ _ _ _ _ => .control
  | .yield _ _ _ => .control
  | .switch _ _ _ => .control
  | .unreachable => .control

/-- Maximum effect in a block: join of all instruction effects and the terminator effect. -/
def blockEffect (b : Block) : Effect :=
  let instrEff := b.instrs.foldl (fun acc i => Effect.join acc (instrEffect i)) .pure
  Effect.join instrEff (termEffect b.term)

/-- All expressions in the current model are pure. -/
theorem exprEffect_pure (e : Expr) : exprEffect e = .pure := by
  induction e with
  | val _ => rfl
  | var _ => rfl
  | bin _ a b iha ihb => simp [exprEffect, iha, ihb]; rfl
  | un _ a iha => simp [exprEffect, iha]

/-- All instructions in the current model are pure. -/
theorem instrEffect_pure (i : Instr) : instrEffect i = .pure := by
  simp [instrEffect, exprEffect_pure]

end MoltTIR
