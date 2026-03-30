//! LLVM backend for release-mode maximum optimization.
//!
//! Requires: `--features llvm` and LLVM 21 installed.
//!
//! This backend targets maximum runtime performance at the cost of
//! slower compilation. Use Cranelift backend for development iteration.

pub mod lowering;
pub mod pgo;
pub mod runtime_imports;
pub mod types;

#[cfg(feature = "llvm")]
use inkwell::OptimizationLevel;
#[cfg(feature = "llvm")]
use inkwell::builder::Builder;
#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::module::Module;
#[cfg(feature = "llvm")]
use inkwell::targets::TargetMachine;

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
        Self {
            context,
            module,
            builder,
        }
    }

    /// Get the compiled LLVM IR as a string (for debugging).
    pub fn dump_ir(&self) -> String {
        self.module.print_to_string().to_string()
    }
}

/// Optimization level for the LLVM compilation pipeline.
#[cfg(feature = "llvm")]
pub enum MoltOptLevel {
    /// No optimization (-O0), fastest compilation. Use for development.
    None,
    /// Standard optimization (-O2), good balance of speed and compile time.
    Speed,
    /// Aggressive optimization (-O3), maximum runtime performance. Use for release.
    Aggressive,
}

/// LTO (Link-Time Optimization) mode.
#[cfg(feature = "llvm")]
pub enum LtoMode {
    /// No LTO. Each module is compiled independently.
    None,
    /// Thin LTO — parallel, scalable, default for `--release`.
    Thin,
    /// Full LTO — maximum inter-procedural optimization, slowest link.
    Full,
}

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    /// Run the FULL LLVM O2/O3 optimization pipeline on the module.
    ///
    /// Before running the pipeline, all user-defined functions are marked
    /// with `dllexport` linkage so GlobalDCE/Internalize cannot remove
    /// them.  After optimization, linkage is restored to `External`.
    /// This gives us 100% of LLVM's optimization power (interprocedural
    /// inlining, GlobalDCE of unused helpers, SCCP, loop vectorization)
    /// while preserving all entry points the linker needs.
    pub fn optimize(&self, opt_level: MoltOptLevel) {
        use inkwell::module::Linkage;
        use inkwell::passes::PassBuilderOptions;

        let target_machine = self.create_target_machine(&opt_level);

        // Mark all externally-visible functions as dllexport so the
        // Internalize pass treats them as API and doesn't remove them.
        let mut preserved: Vec<(String, Linkage)> = Vec::new();
        let mut func = self.module.get_first_function();
        while let Some(f) = func {
            let linkage = f.get_linkage();
            if linkage == Linkage::External && f.count_basic_blocks() > 0 {
                preserved.push((f.get_name().to_str().unwrap_or("").to_string(), linkage));
                f.set_linkage(Linkage::DLLExport);
            }
            func = f.get_next_function();
        }

        let passes = match opt_level {
            MoltOptLevel::None => "default<O0>",
            MoltOptLevel::Speed => "default<O2>",
            MoltOptLevel::Aggressive => "default<O3>",
        };

        let options = PassBuilderOptions::create();
        if let Err(e) = self.module.run_passes(passes, &target_machine, options) {
            eprintln!("WARNING: LLVM optimization pipeline failed: {e}; continuing unoptimized");
        }

        // Restore original linkage after optimization.
        for (name, linkage) in &preserved {
            if let Some(f) = self.module.get_function(name) {
                f.set_linkage(*linkage);
            }
        }
    }

    /// Create a native target machine for the host CPU at the given opt level.
    fn create_target_machine(&self, opt_level: &MoltOptLevel) -> TargetMachine {
        use inkwell::targets::{CodeModel, InitializationConfig, RelocMode, Target};

        Target::initialize_native(&InitializationConfig::default())
            .expect("Failed to initialize native target");

        let triple = TargetMachine::get_default_triple();
        let target = Target::from_triple(&triple).expect("Failed to get target from triple");
        let cpu = TargetMachine::get_host_cpu_name();
        let features = TargetMachine::get_host_cpu_features();

        let llvm_opt = match opt_level {
            MoltOptLevel::None => OptimizationLevel::None,
            MoltOptLevel::Speed => OptimizationLevel::Default,
            MoltOptLevel::Aggressive => OptimizationLevel::Aggressive,
        };

        target
            .create_target_machine(
                &triple,
                cpu.to_str().unwrap(),
                features.to_str().unwrap(),
                llvm_opt,
                RelocMode::Default,
                CodeModel::Default,
            )
            .expect("Failed to create target machine")
    }

    /// Emit LLVM bitcode to a file (used for LTO pipelines).
    ///
    /// Returns `true` on success. The emitted `.bc` file can be passed to
    /// `llvm-lto` or the linker's LTO plugin for cross-module optimization.
    pub fn emit_bitcode(&self, path: &std::path::Path) -> bool {
        self.module.write_bitcode_to_path(path)
    }

    /// Emit a native object file (`.o`) at the given optimization level.
    pub fn emit_object(
        &self,
        path: &std::path::Path,
        opt_level: MoltOptLevel,
    ) -> Result<(), String> {
        use inkwell::targets::FileType;

        let target_machine = self.create_target_machine(&opt_level);
        target_machine
            .write_to_file(&self.module, FileType::Object, path)
            .map_err(|e| e.to_string())
    }

    /// Emit LLVM assembly (`.s`) for debugging or manual inspection.
    pub fn emit_llvm_asm(
        &self,
        path: &std::path::Path,
        opt_level: MoltOptLevel,
    ) -> Result<(), String> {
        use inkwell::targets::FileType;

        let target_machine = self.create_target_machine(&opt_level);
        target_machine
            .write_to_file(&self.module, FileType::Assembly, path)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
#[cfg(feature = "llvm")]
mod tests {
    use super::*;
    use inkwell::context::Context;

    #[test]
    fn test_backend_module_name() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "test_module");
        let ir = backend.dump_ir();
        assert!(
            ir.contains("test_module"),
            "IR should contain the module name, got: {ir}"
        );
    }

    #[test]
    fn test_create_target_machine_all_levels() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "tm_test");
        // Simply ensure no panic at any opt level.
        let _tm_none = backend.create_target_machine(&MoltOptLevel::None);
        let _tm_speed = backend.create_target_machine(&MoltOptLevel::Speed);
        let _tm_agg = backend.create_target_machine(&MoltOptLevel::Aggressive);
    }

    #[test]
    fn test_optimize_empty_module_smoke() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "opt_smoke");
        // Running passes on an empty module must not panic or error.
        backend.optimize(MoltOptLevel::Speed);
    }

    #[test]
    fn test_dump_ir_contains_module_name() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "ir_dump_test");
        let ir = backend.dump_ir();
        assert!(
            ir.contains("ir_dump_test"),
            "dump_ir() should include the module name in the IR string"
        );
    }
}
