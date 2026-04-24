//! End-to-end MLIR compilation pipeline.
//!
//! Provides the top-level `compile_via_mlir` function that the main backend
//! calls. This orchestrates the complete flow:
//!
//! TIR function
//!   -> Build MLIR module (func/arith/cf dialects)
//!   -> Run optimization passes (canonicalize, CSE, SCCP, LICM, inlining)
//!   -> Lower to LLVM dialect (SCF->CF, then all->LLVM)
//!   -> Extract LLVM dialect text (or JIT via ExecutionEngine)
//!
//! The pipeline can also be used incrementally: build and optimize a module,
//! then later lower and JIT separately.

use melior::{
    Context as MlirContext,
    ExecutionEngine,
    ir::operation::OperationLike,
};

use molt_backend::tir::function::TirFunction;

use crate::{lower, optimize, tir_to_mlir};

/// Options controlling the MLIR compilation pipeline.
#[derive(Debug, Clone)]
pub struct MlirCompileOptions {
    /// Optimization level for MLIR passes.
    pub opt_level: MlirOptLevel,
    /// Whether to emit LLVM dialect text in the result.
    pub emit_llvm_dialect: bool,
}

impl Default for MlirCompileOptions {
    fn default() -> Self {
        Self {
            opt_level: MlirOptLevel::O2,
            emit_llvm_dialect: true,
        }
    }
}

/// MLIR optimization level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlirOptLevel {
    /// No MLIR-level optimization.
    O0,
    /// Basic optimizations (canonicalize + CSE).
    O1,
    /// Full optimization pipeline.
    O2,
    /// Aggressive optimizations.
    O3,
}

impl From<MlirOptLevel> for optimize::OptLevel {
    fn from(level: MlirOptLevel) -> Self {
        match level {
            MlirOptLevel::O0 => optimize::OptLevel::O0,
            MlirOptLevel::O1 => optimize::OptLevel::O1,
            MlirOptLevel::O2 => optimize::OptLevel::O2,
            MlirOptLevel::O3 => optimize::OptLevel::O3,
        }
    }
}

/// Result of the MLIR compilation pipeline.
#[derive(Debug, Clone)]
pub struct MlirCompileResult {
    /// The MLIR IR after building from TIR (before optimization).
    pub standard_mlir_text: String,
    /// The MLIR IR after optimization passes (before LLVM lowering).
    pub optimized_mlir_text: String,
    /// The MLIR IR after lowering to LLVM dialect.
    /// Empty if `emit_llvm_dialect` was false.
    pub llvm_dialect_text: String,
}

/// Run the complete MLIR compilation pipeline on a TIR function.
///
/// This is the main entry point for the MLIR backend. It:
/// 1. Builds an MLIR module from the TIR function
/// 2. Runs optimization passes
/// 3. Lowers to LLVM dialect
/// 4. Returns the textual IR at each stage
///
/// The LLVM dialect text can be fed to LLVM for native code generation,
/// or the caller can use `jit_execute_i64` for JIT execution.
pub fn compile_via_mlir(
    tir_func: &TirFunction,
    options: &MlirCompileOptions,
) -> Result<MlirCompileResult, String> {
    let ctx = crate::create_mlir_context();
    // Allow unregistered dialects for molt.* ops that don't have real dialect
    // registration. These ops are used as placeholders for runtime calls.
    ctx.set_allow_unregistered_dialects(true);

    // Step 1: Build MLIR module from TIR.
    let mut module = tir_to_mlir::build_mlir_module(tir_func, &ctx)?;
    let standard_mlir_text = module.as_operation().to_string();

    // Step 2: Run optimization passes.
    optimize::run_optimization_passes_at_level(&mut module, &ctx, options.opt_level.into())?;
    let optimized_mlir_text = module.as_operation().to_string();

    // Step 3: Lower to LLVM dialect.
    let llvm_dialect_text = if options.emit_llvm_dialect {
        lower::lower_to_llvm_dialect(&mut module, &ctx)?;
        module.as_operation().to_string()
    } else {
        String::new()
    };

    Ok(MlirCompileResult {
        standard_mlir_text,
        optimized_mlir_text,
        llvm_dialect_text,
    })
}

/// Build and optimize an MLIR module without lowering to LLVM.
///
/// Useful when the caller wants to inspect or further process the module
/// before lowering.
pub fn create_optimized_module<'c>(
    tir_func: &TirFunction,
    ctx: &'c MlirContext,
    options: &MlirCompileOptions,
) -> Result<MlirCompileResult, String> {
    ctx.set_allow_unregistered_dialects(true);

    let mut module = tir_to_mlir::build_mlir_module(tir_func, ctx)?;
    let standard_mlir_text = module.as_operation().to_string();

    optimize::run_optimization_passes_at_level(&mut module, ctx, options.opt_level.into())?;
    let optimized_mlir_text = module.as_operation().to_string();

    let llvm_dialect_text = if options.emit_llvm_dialect {
        lower::lower_to_llvm_dialect(&mut module, ctx)?;
        module.as_operation().to_string()
    } else {
        String::new()
    };

    Ok(MlirCompileResult {
        standard_mlir_text,
        optimized_mlir_text,
        llvm_dialect_text,
    })
}

/// JIT-compile a TIR function and execute it with i64 arguments, returning an i64 result.
///
/// This runs the full pipeline (build -> optimize -> lower -> JIT) and invokes
/// the function through MLIR's execution engine. The function must:
/// - Accept only i64 parameters
/// - Return a single i64 value
/// - Have the `llvm.emit_c_interface` attribute (added by our builder)
///
/// The `func_name` must match the TIR function's name.
pub fn jit_execute_i64(
    tir_func: &TirFunction,
    func_name: &str,
    args: &[i64],
) -> Result<i64, String> {
    let ctx = crate::create_mlir_context();
    ctx.set_allow_unregistered_dialects(true);

    // Build, optimize, and lower to LLVM dialect.
    let mut module = tir_to_mlir::build_mlir_module(tir_func, &ctx)?;
    optimize::run_optimization_passes(&mut module, &ctx)?;
    lower::lower_to_llvm_dialect(&mut module, &ctx)?;

    if !module.as_operation().verify() {
        return Err("LLVM dialect module failed verification before JIT".to_string());
    }

    // Create the JIT execution engine with optimization level 2.
    let engine = ExecutionEngine::new(&module, 2, &[], false, false);

    // Build the argument buffer. The C interface wrapper expects:
    // - Pointers to each argument (input)
    // - Pointer to the result (output)
    let mut arg_storage: Vec<i64> = args.to_vec();
    let mut result: i64 = 0;

    let mut arg_ptrs: Vec<*mut ()> = Vec::with_capacity(args.len() + 1);
    for arg in &mut arg_storage {
        arg_ptrs.push(arg as *mut i64 as *mut ());
    }
    arg_ptrs.push(&mut result as *mut i64 as *mut ());

    unsafe {
        engine
            .invoke_packed(func_name, &mut arg_ptrs)
            .map_err(|e| format!("JIT invocation of '{func_name}' failed: {e}"))?;
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_backend::tir::{
        blocks::Terminator,
        function::TirFunction,
        ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp},
        types::TirType,
        values::ValueId,
    };

    fn make_identity_func() -> TirFunction {
        let mut func = TirFunction::new("identity".into(), vec![TirType::I64], TirType::I64);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Return {
            values: vec![ValueId(0)],
        };
        func
    }

    fn make_sub_func() -> TirFunction {
        let mut func =
            TirFunction::new("sub".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Sub,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        func
    }

    #[test]
    fn test_pipeline_identity() {
        let result = jit_execute_i64(&make_identity_func(), "identity", &[99]).unwrap();
        assert_eq!(result, 99);
    }

    #[test]
    fn test_pipeline_sub() {
        let result = jit_execute_i64(&make_sub_func(), "sub", &[100, 58]).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_compile_result_stages() {
        let func =
            TirFunction::new("empty_ret".into(), vec![], TirType::I64);
        // This will use the Unreachable terminator default which emits a zero return.
        let result = compile_via_mlir(
            &func,
            &MlirCompileOptions {
                opt_level: MlirOptLevel::O1,
                emit_llvm_dialect: true,
            },
        )
        .unwrap();

        // All stages should produce non-empty text.
        assert!(!result.standard_mlir_text.is_empty());
        assert!(!result.optimized_mlir_text.is_empty());
        assert!(!result.llvm_dialect_text.is_empty());

        // Standard should have func.func, LLVM should have llvm.
        assert!(result.standard_mlir_text.contains("func.func"));
        assert!(result.llvm_dialect_text.contains("llvm."));
    }

    #[test]
    fn test_default_options() {
        let opts = MlirCompileOptions::default();
        assert_eq!(opts.opt_level, MlirOptLevel::O2);
        assert!(opts.emit_llvm_dialect);
    }
}
