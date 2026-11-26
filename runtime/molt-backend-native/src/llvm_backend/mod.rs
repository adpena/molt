//! LLVM backend for release-mode maximum optimization.
//!
//! Requires: `--features llvm` and LLVM 21 installed.
//!
//! This backend targets maximum runtime performance at the cost of
//! slower compilation. Use Cranelift backend for development iteration.

mod app_resolver;
#[cfg(feature = "llvm")]
mod execution_abi;
pub mod lowering;
mod module_registry;
mod program_slots;
pub mod runtime_imports;
pub mod types;
mod wasm_entry;

use crate::CodegenTarget;
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
#[cfg(test)]
use crate::representation_plan::ProducerCarrierPlan;
#[cfg(feature = "llvm")]
use molt_ir::linkage_abi::NativeFunctionLinkageAbi;
#[cfg(feature = "llvm")]
pub struct LlvmBackend<'ctx> {
    pub context: &'ctx Context,
    pub module: Module<'ctx>,
    pub builder: Builder<'ctx>,
    /// One exact signature authority for calls, declarations, and provider
    /// definitions. Batch workers receive these rows from the whole-program
    /// module context before bodies are partitioned.
    pub function_linkage_abis: BTreeMap<String, NativeFunctionLinkageAbi>,
    /// The function table of the document being lowered (HF-847 slice 1): a
    /// typed callee names its linker symbol here.
    pub function_table: molt_ir::FunctionTable,
    /// Overrides for standalone instruction-selection fixtures.
    #[cfg(test)]
    pub(crate) function_repr_facts: BTreeMap<String, ProducerCarrierPlan>,
    /// The runtime entries the linked runtime staticlib defines (the active
    /// stdlib profile's callable surface, from `MOLT_RUNTIME_CALLABLE_SYMBOLS`),
    /// decoded once into a typed set. Runtime calls confirm their entry exists
    /// before emitting an external call to it, so an unavailable entry fails the
    /// build loud instead of silently degrading to an operand-0 pass-through.
    /// Empty when the env var is unset (e.g. in-crate codegen unit tests that
    /// never link a final binary); the generic fallback then declines, matching
    /// the resolver machinery's same `cfg(test)` carve-out.
    pub(crate) runtime_callable_symbols: molt_ir::runtime_callable_symbols::RuntimeFnSet,
    /// This module's runtime declarations, by typed identity (HF-821).
    pub(crate) runtime_imports: runtime_imports::LlvmRuntimeImports<'ctx>,
    /// The machine this module is generated for. The triple, data layout,
    /// relocation model and CPU policy all derive from it.
    pub target: CodegenTarget,
    optimization: MoltOptLevel,
    machine: TargetMachine,
}

impl CodegenTarget {
    /// Create the module's target machine for layout, optimization, and emission.
    fn create_target_machine(self, optimization: MoltOptLevel) -> TargetMachine {
        use inkwell::targets::{CodeModel, Target};

        self.initialize();
        let triple = self.triple();
        let target = Target::from_triple(&triple).expect("Failed to get target from triple");
        let (cpu, features) = self.cpu_features();

        target
            .create_target_machine(
                &triple,
                &cpu,
                &features,
                optimization.machine_level(),
                self.reloc_mode(),
                CodeModel::Default,
            )
            .expect("Failed to create target machine")
    }

    /// The target triple LLVM generates for, as cache identity names it.
    pub(crate) fn triple_name(self) -> String {
        self.triple().as_str().to_string_lossy().into_owned()
    }

    fn triple(self) -> inkwell::targets::TargetTriple {
        match self {
            Self::Host => TargetMachine::get_default_triple(),
            Self::Wasm32Wasip1 => inkwell::targets::TargetTriple::create("wasm32-wasip1"),
            Self::Wasm32Unknown => inkwell::targets::TargetTriple::create("wasm32-unknown-unknown"),
        }
    }

    /// CPU name and feature string. WASM matches the runtime module's Rust
    /// codegen policy: the generic CPU without reference types
    /// (`.cargo/config.toml`, `runtime_wasm_build_support.py`).
    pub(crate) fn cpu_features(self) -> (String, String) {
        match self {
            Self::Host => native_cpu_features(),
            Self::Wasm32Wasip1 | Self::Wasm32Unknown => {
                ("generic".to_string(), "-reference-types".to_string())
            }
        }
    }

    /// Every object is position independent. A wasm32 object links either
    /// statically into one module, where wasm-ld resolves its GOT entries, or
    /// as a side module that the host places after the runtime (`-shared`).
    /// One object, and one shared stdlib archive, serves both links.
    fn reloc_mode(self) -> inkwell::targets::RelocMode {
        inkwell::targets::RelocMode::PIC
    }

    fn initialize(self) {
        use inkwell::targets::{InitializationConfig, Target};
        match self {
            Self::Host => Target::initialize_native(&InitializationConfig::default())
                .expect("Failed to initialize native target"),
            Self::Wasm32Wasip1 | Self::Wasm32Unknown => {
                Target::initialize_webassembly(&InitializationConfig::default())
            }
        }
    }
}

/// Exact LLVM CPU inputs shared by target-machine construction and cache identity.
#[cfg(feature = "llvm")]
pub(crate) fn native_cpu_features() -> (String, String) {
    if crate::native_codegen_portable() {
        ("generic".to_string(), String::new())
    } else {
        (
            TargetMachine::get_host_cpu_name()
                .to_str()
                .unwrap()
                .to_string(),
            TargetMachine::get_host_cpu_features()
                .to_str()
                .unwrap()
                .to_string(),
        )
    }
}

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        Self::new_for_target(
            context,
            module_name,
            CodegenTarget::Host,
            MoltOptLevel::from_request(),
        )
    }

    pub fn new_for_target(
        context: &'ctx Context,
        module_name: &str,
        target: CodegenTarget,
        optimization: MoltOptLevel,
    ) -> Self {
        let module = context.create_module(module_name);
        let builder = context.create_builder();
        module.set_triple(&target.triple());
        let symbols = crate::runtime_callable_symbols::RuntimeCallableSymbols::capture();
        let machine = target.create_target_machine(optimization);
        module.set_data_layout(&machine.get_target_data().get_data_layout());
        let backend = Self {
            context,
            module,
            builder,
            function_linkage_abis: BTreeMap::new(),
            function_table: molt_ir::FunctionTable::default(),
            #[cfg(test)]
            function_repr_facts: BTreeMap::new(),
            runtime_callable_symbols: symbols.runtime_fn_set(),
            runtime_imports: runtime_imports::LlvmRuntimeImports::default(),
            target,
            optimization,
            machine,
        };
        backend.ensure_generated_object_abi_anchor();
        backend
    }

    /// The integer type as wide as a target pointer (`usize`).
    pub(crate) fn pointer_sized_int_type(&self) -> inkwell::types::IntType<'ctx> {
        self.context
            .ptr_sized_int_type(&self.machine.get_target_data(), None)
    }

    /// Attach the mandatory, retained relocation to the runtime's exact
    /// generated-object ABI. This belongs to module construction/emission so
    /// every direct LLVM consumer receives the witness; compile drivers cannot
    /// opt out accidentally. The operation is idempotent for all emit paths.
    fn ensure_generated_object_abi_anchor(&self) {
        if self
            .module
            .get_global(crate::GENERATED_OBJECT_ABI_ANCHOR_SYMBOL)
            .is_some()
        {
            return;
        }
        let witness = self.module.add_global(
            self.context.i8_type(),
            None,
            molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL,
        );
        witness.set_linkage(inkwell::module::Linkage::External);
        let pointer_type = self.context.ptr_type(inkwell::AddressSpace::default());
        let anchor = self.module.add_global(
            pointer_type,
            None,
            crate::GENERATED_OBJECT_ABI_ANCHOR_SYMBOL,
        );
        anchor.set_linkage(inkwell::module::Linkage::Internal);
        anchor.set_initializer(&witness.as_pointer_value());
        let used = self
            .module
            .add_global(pointer_type.array_type(1), None, "llvm.used");
        used.set_linkage(inkwell::module::Linkage::Appending);
        used.set_section(Some("llvm.metadata"));
        used.set_initializer(&pointer_type.const_array(&[anchor.as_pointer_value()]));
    }

    /// Get the compiled LLVM IR as a string (for debugging).
    pub fn dump_ir(&self) -> String {
        self.module.print_to_string().to_string()
    }
}

/// Optimization level for the LLVM compilation pipeline.
#[cfg(feature = "llvm")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum MoltOptLevel {
    O0,
    O1,
    O2,
    #[default]
    O3,
}

impl MoltOptLevel {
    pub(crate) fn from_request() -> Self {
        use molt_ir::backend_environment::{BackendEnv, try_var};
        use serde::Deserialize;
        let Some(value) =
            try_var(BackendEnv::MOLT_LLVM_OPT_LEVEL).expect("read LLVM optimization level")
        else {
            return Self::default();
        };
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(&value))
            .unwrap_or_else(|error| panic!("invalid MOLT_LLVM_OPT_LEVEL={value:?}: {error}"))
    }

    fn pipeline(self) -> &'static str {
        match self {
            Self::O0 => "default<O0>",
            Self::O1 => "default<O1>",
            Self::O2 => "default<O2>",
            Self::O3 => "default<O3>",
        }
    }

    fn machine_level(self) -> OptimizationLevel {
        match self {
            Self::O0 => OptimizationLevel::None,
            Self::O1 => OptimizationLevel::Less,
            Self::O2 => OptimizationLevel::Default,
            Self::O3 => OptimizationLevel::Aggressive,
        }
    }
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
    pub fn optimize(&self) -> Result<(), String> {
        // Activate Polly polyhedral optimizer (no-op if already initialized).
        #[cfg(feature = "polly")]
        Self::init_polly_once();

        self.run_optimization_passes(self.optimization.pipeline())
    }

    fn run_optimization_passes(&self, passes: &str) -> Result<(), String> {
        use inkwell::module::Linkage;
        use inkwell::passes::PassBuilderOptions;

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
            .run_passes(passes, &self.machine, options)
            .map_err(|e| format!("LLVM optimization pipeline `{passes}` failed: {e}"));

        // Restore original linkage after optimization.
        for (name, linkage) in &preserved {
            if let Some(f) = self.module.get_function(name) {
                f.set_linkage(*linkage);
            }
        }

        result
    }

    /// Emit the target object for publication by the caller.
    pub fn emit_object(&self) -> Result<Vec<u8>, String> {
        use inkwell::targets::FileType;

        self.ensure_generated_object_abi_anchor();
        let object = self
            .machine
            .write_to_memory_buffer(&self.module, FileType::Object)
            .map_err(|e| e.to_string())?;
        Ok(object.as_slice().to_vec())
    }

    /// Emit LLVM assembly (`.s`) for debugging or manual inspection.
    pub fn emit_llvm_asm(&self, path: &std::path::Path) -> Result<(), String> {
        use inkwell::targets::FileType;

        self.ensure_generated_object_abi_anchor();
        self.machine
            .write_to_file(&self.module, FileType::Assembly, path)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
#[cfg(feature = "llvm")]
mod tests;
