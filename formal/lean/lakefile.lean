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
             `MoltTIR.Passes.LICM, `MoltTIR.Passes.LICMCorrect,
             `MoltTIR.Passes.CSE, `MoltTIR.Passes.CSECorrect,
             `MoltTIR.Passes.GuardHoist, `MoltTIR.Passes.GuardHoistCorrect,
             `MoltTIR.Passes.JoinCanon, `MoltTIR.Passes.JoinCanonCorrect,
             `MoltTIR.Passes.EdgeThread, `MoltTIR.Passes.EdgeThreadCorrect,
             `MoltTIR.Passes.Pipeline, `MoltTIR.Passes.FullPipeline,
             `MoltTIR.SSA.Dominance,
             `MoltTIR.SSA.WellFormedSSA,
             `MoltTIR.SSA.CSEHelpers,
             `MoltTIR.SSA.PassPreservesSSA,
             `MoltTIR.SSA.Properties,
             `MoltTIR.AbstractInterp.Lattice,
             `MoltTIR.AbstractInterp.GaloisConnection,
             `MoltTIR.AbstractInterp.AbsValue,
             `MoltTIR.AbstractInterp.Widening,
             `MoltTIR.Backend.LuauSyntax, `MoltTIR.Backend.LuauEmit,
             `MoltTIR.Backend.LuauSemantics, `MoltTIR.Backend.LuauEnvCorr,
             `MoltTIR.Backend.LuauCorrect,
             `MoltTIR.Runtime.NanBox, `MoltTIR.Runtime.NanBoxCorrect,
             `MoltTIR.Runtime.Refcount,
             `MoltTIR.Optimization.RefcountElision,
             `MoltTIR.Runtime.RCElisionCorrect,
             `MoltTIR.Runtime.WasmNative,
             `MoltTIR.Runtime.WasmABI, `MoltTIR.Runtime.WasmNativeCorrect,
             `MoltTIR.Determinism.CompileDeterminism,
             `MoltTIR.Determinism.CrossPlatform,
             `MoltTIR.Determinism.BuildReproducibility,
             `MoltTIR.Meta.SorryAudit,
             `MoltTIR.Meta.Completeness,
             `MoltTIR.Simulation.Diagram,
             `MoltTIR.Simulation.PassSimulation,
             `MoltTIR.Simulation.Compose,
             `MoltTIR.Simulation.Adequacy,
             `MoltTIR.Simulation.FullChain,
             `MoltTIR.Compilation.ForwardSimulation,
             `MoltTIR.Compilation.CompilationCorrectness,
             `MoltTIR.Validation.TranslationValidation,
             `MoltTIR.Validation.ConstFoldValid,
             `MoltTIR.Validation.SCCPValid,
             `MoltTIR.Validation.DCEValid,
             `MoltTIR.EndToEndProperties,
             `MoltTIR.EndToEnd,
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
