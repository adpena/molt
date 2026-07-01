//! LLVM backend for release-mode maximum optimization.
//!
//! Requires: `--features llvm` and LLVM 21 installed.
//!
//! This backend targets maximum runtime performance at the cost of
//! slower compilation. Use Cranelift backend for development iteration.

mod app_resolver;
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
use std::collections::BTreeMap;

#[cfg(feature = "llvm")]
use crate::representation_plan::LlvmReprFacts;
#[cfg(feature = "llvm")]
use crate::tir::types::TirType;
#[cfg(feature = "llvm")]
pub struct LlvmBackend<'ctx> {
    pub context: &'ctx Context,
    pub module: Module<'ctx>,
    pub builder: Builder<'ctx>,
    pub function_param_types: BTreeMap<String, Vec<TirType>>,
    pub function_return_types: BTreeMap<String, TirType>,
    /// Per-function representation facts derived from the shared
    /// `ScalarRepresentationPlan`, keyed by function name. These drive the
    /// LLVM backend's integer-carrier and container dispatch decisions from the
    /// same typed facts the native/WASM/Luau backends consume.
    pub(crate) function_repr_facts: BTreeMap<String, LlvmReprFacts>,
    /// The set of `molt_*` symbols the linked runtime staticlib defines (the
    /// active stdlib profile's intrinsic surface, from
    /// `MOLT_RUNTIME_INTRINSIC_SYMBOLS`). Used by the generic preserved-op
    /// runtime-call fallback to confirm `molt_<kind>` actually exists before
    /// emitting an external call to it — so an operator kind with no dedicated
    /// lowering routes to its real runtime entry, and an unmappable kind fails
    /// the build loud instead of silently degrading to an operand-0 pass-through.
    /// Empty when the env var is unset (e.g. in-crate codegen unit tests that
    /// never link a final binary); the generic fallback then declines, matching
    /// the resolver machinery's same `cfg(test)` carve-out.
    pub(crate) runtime_intrinsic_symbols: std::collections::BTreeSet<String>,
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
            function_param_types: BTreeMap::new(),
            function_return_types: BTreeMap::new(),
            function_repr_facts: BTreeMap::new(),
            runtime_intrinsic_symbols:
                crate::intrinsic_symbols::runtime_intrinsic_symbols_from_env().unwrap_or_default(),
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
    /// Initialize Polly polyhedral optimization via LLVM command-line flags.
    ///
    /// Polly is an LLVM plugin that performs polyhedral loop optimization:
    /// dependence analysis, tiling, interchange, vectorization via strip-mining.
    /// When the host LLVM is built with Polly support (`-DLLVM_ENABLE_PROJECTS=polly`),
    /// these flags activate it for all subsequent `run_passes` invocations.
    ///
    /// Called once before any compilation via `std::sync::Once`. If Polly is not
    /// available in the LLVM build, the flags are silently ignored — LLVM does not
    /// error on unknown `-mllvm` flags passed through `LLVMParseCommandLineOptions`.
    ///
    /// Flags:
    /// - `-polly`: enable polyhedral optimizer
    /// - `-polly-vectorizer=stripmine`: use strip-mining vectorization (better for
    ///   deep loop nests than the default polly vectorizer)
    /// - `-polly-parallel`: emit parallel code for independent loop iterations
    /// - `-polly-position=early`: run Polly early in the pipeline before LLVM's
    ///   own loop transforms can obscure polyhedral structure
    #[cfg(feature = "polly")]
    fn init_polly_once() {
        use std::sync::Once;
        static POLLY_INIT: Once = Once::new();
        POLLY_INIT.call_once(|| {
            use std::ffi::CString;
            let args: Vec<CString> = [
                "molt-backend",
                "-polly",
                "-polly-vectorizer=stripmine",
                "-polly-parallel",
                "-polly-position=early",
            ]
            .iter()
            .map(|s| CString::new(*s).unwrap())
            .collect();
            let ptrs: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
            let overview = CString::new("Molt LLVM backend with Polly").unwrap();
            unsafe {
                llvm_sys::support::LLVMParseCommandLineOptions(
                    ptrs.len() as libc::c_int,
                    ptrs.as_ptr(),
                    overview.as_ptr(),
                );
            }
        });
    }

    /// Run the FULL LLVM O2/O3 optimization pipeline on the module.
    ///
    /// Before running the pipeline, all user-defined functions are marked
    /// with `dllexport` linkage so GlobalDCE/Internalize cannot remove
    /// them.  After optimization, linkage is restored to `External`.
    /// This gives us 100% of LLVM's optimization power (interprocedural
    /// inlining, GlobalDCE of unused helpers, SCCP, loop vectorization)
    /// while preserving all entry points the linker needs.
    ///
    /// When the `polly` feature is enabled and the host LLVM includes Polly,
    /// polyhedral loop optimizations (tiling, interchange, strip-mine
    /// vectorization) are applied automatically after the standard O2/O3
    /// pipeline, giving additional gains on loop-heavy numeric code.
    pub fn optimize(&self, opt_level: MoltOptLevel) -> Result<(), String> {
        // Activate Polly polyhedral optimizer (no-op if already initialized).
        #[cfg(feature = "polly")]
        Self::init_polly_once();

        let passes = match opt_level {
            MoltOptLevel::None => "default<O0>",
            MoltOptLevel::Speed => "default<O2>",
            MoltOptLevel::Aggressive => "default<O3>",
        };
        self.run_optimization_passes(&opt_level, passes)
    }

    fn run_optimization_passes(
        &self,
        opt_level: &MoltOptLevel,
        passes: &str,
    ) -> Result<(), String> {
        use inkwell::module::Linkage;
        use inkwell::passes::PassBuilderOptions;

        let target_machine = self.create_target_machine(opt_level);

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

        let options = PassBuilderOptions::create();
        options.set_loop_vectorization(true);
        options.set_loop_slp_vectorization(true);
        options.set_loop_unrolling(true);
        options.set_loop_interleaving(true);
        options.set_merge_functions(true);
        let result = self
            .module
            .run_passes(passes, &target_machine, options)
            .map_err(|e| format!("LLVM optimization pipeline `{passes}` failed: {e}"));

        // Restore original linkage after optimization.
        for (name, linkage) in &preserved {
            if let Some(f) = self.module.get_function(name) {
                f.set_linkage(*linkage);
            }
        }

        result
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
                Self::reloc_mode(),
                CodeModel::Default,
            )
            .expect("Failed to create target machine")
    }

    /// Molt LLVM objects are linked into the same runtime/shared-image path as
    /// native backend outputs, so object generation must be position independent
    /// at the target-machine authority instead of relying on linker flags.
    fn reloc_mode() -> inkwell::targets::RelocMode {
        inkwell::targets::RelocMode::PIC
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
        assert_eq!(
            LlvmBackend::reloc_mode(),
            inkwell::targets::RelocMode::PIC,
            "LLVM object emission must stay PIC-compatible for the shared runtime link path"
        );
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
        backend
            .optimize(MoltOptLevel::Speed)
            .expect("empty-module optimization should succeed");
    }

    #[test]
    fn test_optimize_invalid_pipeline_fails_closed_and_restores_linkage() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "opt_fail_closed");
        let i64_ty = ctx.i64_type();
        let func = backend.module.add_function(
            "visible_entry",
            i64_ty.fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        let entry = ctx.append_basic_block(func, "entry");
        backend.builder.position_at_end(entry);
        backend
            .builder
            .build_return(Some(&i64_ty.const_zero()))
            .unwrap();

        let err = backend
            .run_optimization_passes(&MoltOptLevel::Speed, "not-a-real-pass")
            .expect_err("invalid LLVM pass pipeline must fail closed");
        assert!(
            err.contains("not-a-real-pass"),
            "error should identify the rejected pass pipeline: {err}"
        );
        assert_eq!(
            func.get_linkage(),
            inkwell::module::Linkage::External,
            "temporary dllexport linkage must be restored after optimizer failure"
        );
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
