//! LLVM backend for release-mode maximum optimization.
//!
//! Requires: `--features llvm` and LLVM 19 installed.
//!
//! This backend targets maximum runtime performance at the cost of
//! slower compilation. Use Cranelift backend for development iteration.

pub mod types;
pub mod runtime_imports;
pub mod lowering;

#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::module::Module;
#[cfg(feature = "llvm")]
use inkwell::builder::Builder;
#[cfg(feature = "llvm")]
use inkwell::OptimizationLevel;

#[cfg(feature = "llvm")]
pub struct LlvmBackend<'ctx> {
    pub context: &'ctx Context,
    pub module: Module<'ctx>,
    pub builder: Builder<'ctx>,
}

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        let builder = context.create_builder();
        // Set target triple for the host
        let triple = inkwell::targets::TargetMachine::get_default_triple();
        module.set_triple(&triple);
        Self { context, module, builder }
    }

    /// Get the compiled LLVM IR as a string (for debugging).
    pub fn dump_ir(&self) -> String {
        self.module.print_to_string().to_string()
    }
}
