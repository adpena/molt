import Lake
open Lake DSL

package MoltTIR where
  leanOptions := #[
    ⟨`autoImplicit, false⟩
  ]

@[default_target]
lean_lib MoltTIR where
  srcDir := "."
  roots := #[`MoltTIR.Basic, `MoltTIR.Types, `MoltTIR.Syntax, `MoltTIR.WellFormed,
             `MoltTIR.Semantics.State, `MoltTIR.Semantics.EvalExpr,
             `MoltTIR.Semantics.ExecBlock, `MoltTIR.Semantics.ExecFunc,
             `MoltTIR.Semantics.Determinism,
             `MoltTIR.CFG,
             `MoltTIR.Passes.Effects,
             `MoltTIR.Passes.ConstFold, `MoltTIR.Passes.ConstFoldCorrect,
             `MoltTIR.Passes.DCE, `MoltTIR.Passes.DCECorrect,
             `MoltTIR.Passes.Lattice, `MoltTIR.Passes.SCCP, `MoltTIR.Passes.SCCPCorrect,
             `MoltTIR.Tests.Smoke]

lean_lib MoltPython where
  srcDir := "."
  roots := #[`MoltPython.Syntax, `MoltPython.Values, `MoltPython.Env,
             `MoltPython.Semantics.EvalExpr, `MoltPython.Semantics.Determinism,
             `MoltPython.Properties.TypeSafety]
