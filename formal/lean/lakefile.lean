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
             `MoltTIR.CFG, `MoltTIR.CFG.Loops,
             `MoltTIR.Passes.Effects,
             `MoltTIR.Passes.ConstFold, `MoltTIR.Passes.ConstFoldCorrect,
             `MoltTIR.Passes.DCE, `MoltTIR.Passes.DCECorrect,
             `MoltTIR.Passes.Lattice, `MoltTIR.Passes.SCCP, `MoltTIR.Passes.SCCPCorrect,
             `MoltTIR.Passes.SCCPMulti, `MoltTIR.Passes.SCCPMultiCorrect,
             `MoltTIR.Passes.LICM, `MoltTIR.Passes.LICMCorrect,
             `MoltTIR.Passes.CSE, `MoltTIR.Passes.CSECorrect,
             `MoltTIR.Passes.GuardHoist, `MoltTIR.Passes.GuardHoistCorrect,
             `MoltTIR.Passes.JoinCanon, `MoltTIR.Passes.JoinCanonCorrect,
             `MoltTIR.Passes.EdgeThread, `MoltTIR.Passes.EdgeThreadCorrect,
             `MoltTIR.Passes.Pipeline,
             `MoltTIR.Backend.LuauSyntax, `MoltTIR.Backend.LuauEmit,
             `MoltTIR.Backend.LuauSemantics, `MoltTIR.Backend.LuauEnvCorr,
             `MoltTIR.Backend.LuauCorrect,
             `MoltTIR.Tests.Smoke]
