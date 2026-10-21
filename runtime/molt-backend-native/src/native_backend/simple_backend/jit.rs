//! The development JIT code owner (HF-371, `docs/design/dev_jit_tier.md`).
//!
//! The same codegen that writes shipping objects emits into a
//! `cranelift_jit::JITModule` here. Imported runtime symbols resolve through
//! the caller's lookup (the runner resolves them in the runtime image it
//! loaded), and every import must resolve before any generated code runs.

use super::compile_driver::backend_timing_enabled;
use super::*;
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use molt_ir::backend_environment::BackendEnv;
use std::sync::Arc;

/// Resolves an imported symbol to its address in the current process.
pub type JitSymbolLookup = Arc<dyn Fn(&str) -> Option<*const u8> + Send + Sync>;

/// Address space reserved for one program's code and data. The reservation is
/// `PROT_NONE` until codegen asks for pages, so it costs no memory. One
/// contiguous arena keeps every PC-relative reference between generated code
/// and generated data in range (AArch64 `adrp` reaches +/-4 GiB; calls out of
/// branch range go through `cranelift_jit` veneers).
const JIT_ARENA_RESERVATION_BYTES: usize = 1 << 30;

pub(crate) struct JitCodeOwner {
    pub(crate) module: JITModule,
    lookup: JitSymbolLookup,
}

/// The program entry the AOT main stub calls (`molt_main`, emitted by the
/// frontend's entry module lowering).
const PROGRAM_ENTRY_SYMBOL: &str = "molt_main";
/// The application initializer the runtime calls for a fresh isolate.
const ISOLATE_BOOTSTRAP_SYMBOL: &str = "molt_isolate_bootstrap";

/// A finalized program in executable memory. The memory lives as long as the
/// process: the runner leaves the program through the runtime's process exit.
pub struct JitProgram {
    module: JITModule,
}

/// The program symbols a launcher hands to the runtime, as the AOT main stub
/// does: the per-app callable resolver, the module registry blob, the entry
/// and the isolate initializer.
#[derive(Clone, Copy, Debug)]
pub struct JitEntryPoints {
    /// `molt_app_resolve_callable(name_ptr, name_len) -> fn_ptr`.
    pub app_callable_resolver: *const u8,
    /// `molt_module_registry_blob`.
    pub module_registry_blob: *const u8,
    /// `molt_main()`.
    pub main: *const u8,
    /// `molt_isolate_bootstrap() -> u64`.
    pub isolate_bootstrap: *const u8,
}

impl JitProgram {
    /// Every launch symbol, or the first one this program does not define.
    pub fn entry_points(&self) -> Result<JitEntryPoints, JitCompileError> {
        let function = |name: &'static str| {
            self.function_address(name)
                .ok_or(JitCompileError::MissingProgramSymbol(name))
        };
        Ok(JitEntryPoints {
            app_callable_resolver: function(crate::app_resolver_abi::APP_RESOLVER_SYMBOL)?,
            module_registry_blob: self
                .data_address(super::module_registry::MODULE_REGISTRY_BLOB_SYMBOL)
                .ok_or(JitCompileError::MissingProgramSymbol(
                    super::module_registry::MODULE_REGISTRY_BLOB_SYMBOL,
                ))?,
            main: function(PROGRAM_ENTRY_SYMBOL)?,
            isolate_bootstrap: function(ISOLATE_BOOTSTRAP_SYMBOL)?,
        })
    }

    /// The address of a function this program defines.
    pub fn function_address(&self, name: &str) -> Option<*const u8> {
        match self.module.get_name(name)? {
            cranelift_module::FuncOrDataId::Func(id)
                if self
                    .module
                    .declarations()
                    .get_function_decl(id)
                    .linkage
                    .is_definable() =>
            {
                Some(self.module.get_finalized_function(id))
            }
            _ => None,
        }
    }

    /// The address of a data object this program defines.
    pub fn data_address(&self, name: &str) -> Option<*const u8> {
        match self.module.get_name(name)? {
            cranelift_module::FuncOrDataId::Data(id)
                if self
                    .module
                    .declarations()
                    .get_data_decl(id)
                    .linkage
                    .is_definable() =>
            {
                Some(self.module.get_finalized_data(id).0)
            }
            _ => None,
        }
    }
}

/// Why a program cannot run in the JIT tier. Each variant names the construct.
#[derive(Debug)]
pub enum JitCompileError {
    /// The program failed the op-shape or program-slot admission that every
    /// native code owner applies.
    Admission(molt_ir::ir_schema::FunctionOpShapeDiagnostic),
    /// The JIT tier compiles with Cranelift only.
    UnsupportedCodegenBackend(String),
    /// Generated code imports symbols the loaded runtime does not define.
    UnresolvedImports(Vec<String>),
    /// The program defines no symbol the launcher needs.
    MissingProgramSymbol(&'static str),
    /// Generated code did not return what it was compiled to return.
    ProbeMismatch { expected: i64, actual: i64 },
    /// `cranelift_jit` could not make the code executable.
    Finalize(String),
}

impl std::fmt::Display for JitCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admission(diagnostic) => write!(f, "program admission failed: {diagnostic}"),
            Self::UnsupportedCodegenBackend(backend) => write!(
                f,
                "the JIT tier compiles with Cranelift; MOLT_BACKEND={backend} is not supported"
            ),
            Self::UnresolvedImports(names) => write!(
                f,
                "the runtime defines none of these {} imported symbol(s): {}",
                names.len(),
                names.join(", ")
            ),
            Self::MissingProgramSymbol(name) => {
                write!(f, "the program defines no `{name}` launch symbol")
            }
            Self::ProbeMismatch { expected, actual } => write!(
                f,
                "generated probe code returned {actual:#x}, expected {expected:#x}"
            ),
            Self::Finalize(error) => write!(f, "cannot finalize JIT code: {error}"),
        }
    }
}

impl std::error::Error for JitCompileError {}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    /// A backend whose code owner is executable memory in this process, for
    /// the host target. `lookup` resolves every imported symbol.
    pub fn new_jit(lookup: JitSymbolLookup) -> Self {
        let isa = Self::native_isa(None, CodePlacement::ProcessMemory);
        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let resolve = Arc::clone(&lookup);
        builder.symbol_lookup_fn(Box::new(move |name| resolve(name)));
        let arena = ArenaMemoryProvider::new_with_size(JIT_ARENA_RESERVATION_BYTES).unwrap_or_else(
            |error| {
                panic!("cannot reserve {JIT_ARENA_RESERVATION_BYTES} bytes for JIT code: {error}")
            },
        );
        builder.memory_provider(Box::new(arena));
        Self::with_code_owner(CodeOwner::ProcessMemory(Box::new(JitCodeOwner {
            module: JITModule::new(builder),
            lookup,
        })))
    }

    /// Compile a whole program into executable memory. The program carries its
    /// own app resolver, module registry and program slots, as the single
    /// application object of an AOT build does.
    pub fn compile_jit(mut self, ir: SimpleIR) -> Result<JitProgram, JitCompileError> {
        if let Some(backend) = env_setting(BackendEnv::MOLT_BACKEND)
            && backend == "llvm"
        {
            return Err(JitCompileError::UnsupportedCodegenBackend(backend));
        }
        self.admit_program(&ir)
            .map_err(JitCompileError::Admission)?;
        let timing = backend_timing_enabled();
        let compile_start = std::time::Instant::now();
        let mut ir = ir;
        let program = self.prepare_native_codegen(&mut ir, false, timing, &compile_start);
        let (func_count, total_ops) = self.emit_cranelift_program(program, timing);
        let SimpleBackend { module, .. } = self;
        let CodeOwner::ProcessMemory(owner) = module else {
            panic!("JIT compilation requires a process-memory code owner");
        };
        let finalize_start = std::time::Instant::now();
        let program = (*owner).finalize()?;
        if timing {
            eprintln!(
                "MOLT_BACKEND_TIMING: JIT finalize took {:.2?}",
                finalize_start.elapsed()
            );
            eprintln!(
                "MOLT_BACKEND_TIMING: total JIT compile: {:.2?} ({func_count} functions, {total_ops} ops)",
                compile_start.elapsed()
            );
        }
        Ok(program)
    }
}

impl JitCodeOwner {
    /// Every import must resolve before relocation; `cranelift_jit` would
    /// otherwise panic on the first missing name, after part of the program
    /// was already patched.
    fn finalize(mut self) -> Result<JitProgram, JitCompileError> {
        let declarations = self.module.declarations();
        let function_imports = declarations
            .get_functions()
            .filter(|(_, declaration)| declaration.linkage == Linkage::Import)
            .filter_map(|(_, declaration)| declaration.name.as_deref());
        let data_imports = declarations
            .get_data_objects()
            .filter(|(_, declaration)| declaration.linkage == Linkage::Import)
            .filter_map(|(_, declaration)| declaration.name.as_deref());
        let mut unresolved: Vec<String> = function_imports
            .chain(data_imports)
            .filter(|name| (self.lookup)(name).is_none())
            .map(str::to_owned)
            .collect();
        if !unresolved.is_empty() {
            unresolved.sort();
            unresolved.dedup();
            return Err(JitCompileError::UnresolvedImports(unresolved));
        }
        self.module
            .finalize_definitions()
            .map_err(|error| JitCompileError::Finalize(error.to_string()))?;
        Ok(JitProgram {
            module: self.module,
        })
    }
}

/// Prove that this process can write, protect and run generated code (W^X on
/// Apple Silicon, icache coherence everywhere) with the ISA and memory owner a
/// program uses. The runner's build admission runs it once per binary.
#[cfg(feature = "native-backend")]
pub fn probe_jit_execution() -> Result<(), JitCompileError> {
    const PROBE_SYMBOL: &str = "molt_jit_probe";
    const PROBE_VALUE: i64 = i64::from_be_bytes(*b"MoltJIT!");
    let backend = SimpleBackend::new_jit(Arc::new(|_| None));
    let CodeOwner::ProcessMemory(owner) = backend.module else {
        panic!("the JIT probe requires a process-memory code owner");
    };
    let mut owner = *owner;
    let module = &mut owner.module;
    let mut signature = module.make_signature();
    signature.returns.push(AbiParam::new(types::I64));
    let id = module
        .declare_function(PROBE_SYMBOL, Linkage::Export, &signature)
        .map_err(|error| JitCompileError::Finalize(error.to_string()))?;
    let mut context = module.make_context();
    context.func.signature = signature;
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let value = builder.ins().iconst(types::I64, PROBE_VALUE);
        builder.ins().return_(&[value]);
        builder.finalize(module.target_config());
    }
    module
        .define_function(id, &mut context)
        .map_err(|error| JitCompileError::Finalize(error.to_string()))?;
    let program = owner.finalize()?;
    let address = program
        .function_address(PROBE_SYMBOL)
        .ok_or(JitCompileError::MissingProgramSymbol(PROBE_SYMBOL))?;
    // SAFETY: `address` is the finalized code of the function defined above,
    // whose signature is `extern "C" fn() -> i64` in the host call convention.
    let probe: extern "C" fn() -> i64 = unsafe { std::mem::transmute(address) };
    let actual = probe();
    if actual != PROBE_VALUE {
        return Err(JitCompileError::ProbeMismatch {
            expected: PROBE_VALUE,
            actual,
        });
    }
    Ok(())
}
