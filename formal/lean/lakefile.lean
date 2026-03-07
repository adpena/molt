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
             `MoltTIR.Semantics.BlockCorrect, `MoltTIR.Semantics.FuncCorrect,
             `MoltTIR.CFG, `MoltTIR.CFG.Loops,
             `MoltTIR.Passes.Effects,
             `MoltTIR.Passes.ConstFold, `MoltTIR.Passes.ConstFoldCorrect,
             `MoltTIR.Passes.DCE, `MoltTIR.Passes.DCECorrect,
             `MoltTIR.Passes.Lattice, `MoltTIR.Passes.SCCP, `MoltTIR.Passes.SCCPCorrect,
             `MoltTIR.Passes.SCCPMulti, `MoltTIR.Passes.SCCPMultiCorrect,
             `MoltTIR.Passes.CSE, `MoltTIR.Passes.CSECorrect,
             `MoltTIR.Passes.LICM, `MoltTIR.Passes.LICMCorrect,
             `MoltTIR.Passes.Pipeline,
             `MoltTIR.Runtime.NanBox, `MoltTIR.Runtime.Refcount, `MoltTIR.Runtime.WasmNative,
             `MoltTIR.Backend.LuauSyntax, `MoltTIR.Backend.LuauEmit, `MoltTIR.Backend.LuauCorrect,
             `MoltTIR.Tests.Smoke]
