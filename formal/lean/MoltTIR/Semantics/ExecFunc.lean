/-
  MoltTIR.Semantics.ExecFunc — function-level execution with fuel-bounded stepping.

  Uses a fuel parameter to avoid divergence in the formalization.
  Later work can model divergence explicitly via coinduction or prove
  termination for specific program classes.
-/
import MoltTIR.Semantics.ExecBlock

namespace MoltTIR

/-- Execute a function starting at a given label with a given environment.
    `fuel` bounds the number of block transitions to prevent divergence. -/
def execFunc (f : Func) : Nat → Env → Label → Option Outcome
  | 0, _, _ => none  -- out of fuel (neither stuck nor returned)
  | fuel + 1, ρ, lbl =>
      match f.blocks lbl with
      | none => some .stuck
      | some blk =>
          match execInstrs ρ blk.instrs with
          | none => some .stuck
          | some ρ' =>
              match evalTerminator f ρ' blk.term with
              | none => some .stuck
              | some (.ret v) => some (.ret v)
              | some (.jump target env') => execFunc f fuel env' target

/-- Top-level entry point: execute a function from its entry block with empty env. -/
def runFunc (f : Func) (fuel : Nat) : Option Outcome :=
  match f.blocks f.entry with
  | none => some .stuck
  | some blk =>
      -- Entry block should have no params (or we'd need initial args)
      if blk.params.isEmpty then
        execFunc f fuel Env.empty f.entry
      else
        some .stuck

/-- execFunc is deterministic: same inputs → same output.
    This follows trivially from execFunc being a total function (not a relation). -/
theorem execFunc_deterministic (f : Func) (fuel : Nat) (ρ : Env) (lbl : Label) :
    ∀ o1 o2, execFunc f fuel ρ lbl = some o1 → execFunc f fuel ρ lbl = some o2 → o1 = o2 := by
  intro o1 o2 h1 h2
  simp [h1] at h2
  exact h2

end MoltTIR
