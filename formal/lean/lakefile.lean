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
             `MoltTIR.Passes.FullPipeline,
             `MoltTIR.Termination.PassTermination,
             `MoltTIR.Termination.PipelineTermination,
             `MoltTIR.SSA.Dominance,
             `MoltTIR.SSA.WellFormedSSA,
             `MoltTIR.SSA.PassPreservesSSA,
             `MoltTIR.SSA.Properties,
             `MoltTIR.AbstractInterp.Lattice,
             `MoltTIR.AbstractInterp.GaloisConnection,
             `MoltTIR.AbstractInterp.AbsValue,
             `MoltTIR.AbstractInterp.Widening,
             `MoltTIR.Backend.LuauSyntax, `MoltTIR.Backend.LuauEmit,
             `MoltTIR.Backend.LuauSemantics, `MoltTIR.Backend.LuauEnvCorr,
             `MoltTIR.Backend.LuauCorrect,
             `MoltTIR.Backend.WasmSyntax, `MoltTIR.Backend.WasmEmit,
             `MoltTIR.Backend.WasmCorrect,
             `MoltTIR.Runtime.NanBox, `MoltTIR.Runtime.Refcount,
             `MoltTIR.Runtime.WasmNative,
             `MoltTIR.Runtime.WasmABI, `MoltTIR.Runtime.WasmNativeCorrect,
             `MoltTIR.Determinism.CompileDeterminism,
             `MoltTIR.Determinism.CrossPlatform,
             `MoltTIR.Determinism.BuildReproducibility,
             `MoltTIR.Tests.Smoke]

lean_lib MoltPython where
  srcDir := "."
  roots := #[`MoltPython.Syntax, `MoltPython.Values, `MoltPython.Env,
             `MoltPython.Semantics.EvalExpr, `MoltPython.Semantics.Determinism,
             `MoltPython.Properties.TypeSafety]

lean_lib MoltLowering where
  srcDir := "."
  roots := #[`MoltLowering.ASTtoTIR, `MoltLowering.Properties,
             `MoltLowering.Correct]
