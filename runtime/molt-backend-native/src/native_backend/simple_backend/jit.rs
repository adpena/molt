//! The development JIT code owner (HF-371, `docs/design/dev_jit_tier.md`).
//!
//! The same codegen that writes shipping objects emits into a
//! `cranelift_jit::JITModule` here. Imported runtime symbols resolve through
//! the caller's lookup (the runner resolves them in the runtime image it
//! loaded), and every import must resolve before relocating code that uses it.

use super::compile_driver::{NativeCodegenProgram, backend_timing_enabled};
use super::machine_code_cache::MachineCodeCache;
use super::*;
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use molt_ir::PROGRAM_ENTRY_SYMBOL;
use molt_ir::backend_environment::BackendEnv;
use std::sync::Arc;

mod lazy;

/// Resolves an imported symbol to its address in the current process.
pub type JitSymbolLookup = Arc<dyn Fn(&str) -> Option<*const u8> + Send + Sync>;

/// Status used by the runner for pre-launch and first-call compilation failures.
pub const JIT_FAILURE_STATUS: i32 = 125;

/// Address space reserved for one program's code and data. The reservation is
/// `PROT_NONE` until codegen asks for pages, so it costs no memory. One
/// contiguous arena keeps every PC-relative reference between generated code
/// and generated data in range (AArch64 `adrp` reaches +/-4 GiB; calls out of
/// branch range go through `cranelift_jit` veneers).
const JIT_ARENA_RESERVATION_BYTES: usize = 1 << 30;

pub(crate) struct JitCodeOwner {
    pub(crate) module: JITModule,
    machine_cache: Option<MachineCodeCache>,
    lookup: JitSymbolLookup,
    code: Box<lazy::LazyCode>,
    deferred: BTreeMap<cranelift_module::FuncId, lazy::Entry>,
}

/// The application initializer the runtime calls for a fresh isolate.
const ISOLATE_BOOTSTRAP_SYMBOL: &str = "molt_isolate_bootstrap";

/// A program with finalized entry points in executable memory. The runner
/// retains this owner, including first-call state, until process exit.
pub struct JitProgram {
    code: Box<lazy::LazyCode>,
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
    /// `molt_module_ready`: the registry's writable ready block.
    pub module_ready: *mut u64,
    /// The schema-bound compiled main, `(context) -> void`.
    pub main: *const u8,
    /// The schema-bound compiled isolate initializer, `(context) -> u64`.
    pub isolate_bootstrap: *const u8,
}

impl JitProgram {
    #[cfg(test)]
    pub(super) fn validate_body_relocations(&self) -> Result<(), JitCompileError> {
        self.code.materialize_all()
    }

    /// A shared stdlib and its JIT application use the same ready block.
    pub fn entry_points(
        &self,
        ready: std::ptr::NonNull<u64>,
    ) -> Result<JitEntryPoints, JitCompileError> {
        let function = |name: &'static str| {
            self.declared_function_address(name)
                .ok_or(JitCompileError::MissingProgramSymbol(name))
        };
        Ok(JitEntryPoints {
            app_callable_resolver: function(crate::app_resolver_abi::APP_RESOLVER_SYMBOL)?,
            module_registry_blob: self
                .data_address(super::module_registry::MODULE_REGISTRY_BLOB_SYMBOL)
                .ok_or(JitCompileError::MissingProgramSymbol(
                    super::module_registry::MODULE_REGISTRY_BLOB_SYMBOL,
                ))?,
            module_ready: ready.as_ptr(),
            main: self
                .function_address(PROGRAM_ENTRY_SYMBOL)
                .ok_or(JitCompileError::MissingProgramSymbol(PROGRAM_ENTRY_SYMBOL))?,
            isolate_bootstrap: self.function_address(ISOLATE_BOOTSTRAP_SYMBOL).ok_or(
                JitCompileError::MissingProgramSymbol(ISOLATE_BOOTSTRAP_SYMBOL),
            )?,
        })
    }

    /// The address of a function this program defines.
    /// Callers must retain this program while they use a returned address.
    pub fn function_address(&self, name: &str) -> Option<*const u8> {
        self.declared_function_address(&compiled_execution_symbol(name))
    }

    /// Exact physical binding for a genuine host entry, such as the resolver
    /// or the W^X probe. Python callables use `function_address` instead.
    fn declared_function_address(&self, name: &str) -> Option<*const u8> {
        self.code
            .with_module(|module| match module.get_name(name)? {
                cranelift_module::FuncOrDataId::Func(id)
                    if module
                        .declarations()
                        .get_function_decl(id)
                        .linkage
                        .is_definable() =>
                {
                    Some(module.get_finalized_function(id))
                }
                _ => None,
            })
    }

    /// The address of a data object this program defines.
    pub fn data_address(&self, name: &str) -> Option<*const u8> {
        self.code
            .with_module(|module| match module.get_name(name)? {
                cranelift_module::FuncOrDataId::Data(id)
                    if module
                        .declarations()
                        .get_data_decl(id)
                        .linkage
                        .is_definable() =>
                {
                    Some(module.get_finalized_data(id).0)
                }
                _ => None,
            })
    }
}

/// Why a program cannot run in the JIT tier. Each variant names the construct.
#[derive(Debug)]
pub enum JitCompileError {
    /// The program failed the op-shape or program-slot admission that every
    /// native code owner applies.
    Admission(molt_ir::ir_schema::FunctionOpShapeDiagnostic),
    /// A function reference in the incoming document cannot be resolved.
    FunctionReferences(String),
    /// A final TIR body was refused before it could execute.
    Lowering(Vec<String>),
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
            Self::Lowering(errors) => {
                write!(f, "Cranelift lowering failed:\n{}", errors.join("\n"))
            }
            Self::FunctionReferences(message) => {
                write!(f, "program function references: {message}")
            }
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
        builder.symbol(lazy::MATERIALIZE_SYMBOL, lazy::materializer_address());
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
            machine_cache: None,
            lookup,
            code: Box::default(),
            deferred: BTreeMap::new(),
        })))
    }

    /// Prepare declarations, executable entries and application metadata.
    /// Each local function's entry assembles its body on first call.
    pub fn compile_jit(mut self, ir: SimpleIR) -> Result<JitProgram, JitCompileError> {
        Self::admit_jit_codegen()?;
        let mut ir = ir;
        ir.resolve_function_references()
            .map_err(JitCompileError::FunctionReferences)?;
        self.admit_program(&ir)
            .map_err(JitCompileError::Admission)?;
        let timing = backend_timing_enabled();
        let compile_start = std::time::Instant::now();
        let program = self.prepare_native_codegen(&mut ir, false, true, timing, &compile_start);
        self.stage_jit_program(program, timing, compile_start)
    }

    /// Stage stable entries for the application and its specialized clones.
    /// Canonical stdlib entries resolve through the loaded image.
    pub fn compile_prepared_jit(
        mut self,
        application: PreparedJitApplication,
    ) -> Result<JitProgram, JitCompileError> {
        Self::admit_jit_codegen()?;
        let start = std::time::Instant::now();
        let PreparedJitApplication {
            declarations,
            bodies,
            registry,
            linkage,
            callables,
        } = application;
        let ir = SimpleIR {
            functions: declarations,
            profile: None,
        };
        let final_functions =
            super::module_metadata::NativeFinalFunctions::from_jit_artifact(ir, bodies)
                .map_err(JitCompileError::Finalize)?;
        final_functions
            .validate_extern_calls()
            .map_err(JitCompileError::Finalize)?;
        self.external_program_storage = true;
        self.module_registry = Some(registry);
        self.set_module_context(linkage);
        self.stage_jit_program(
            super::compile_driver::NativeCodegenProgram {
                emit_resolver_here: true,
                app_callable_manifest: callables,
                final_functions: Some(final_functions),
            },
            backend_timing_enabled(),
            start,
        )
    }

    fn admit_jit_codegen() -> Result<(), JitCompileError> {
        if let Some(backend) = env_setting(BackendEnv::MOLT_BACKEND)
            && backend == "llvm"
        {
            return Err(JitCompileError::UnsupportedCodegenBackend(backend));
        }
        Ok(())
    }

    fn stage_jit_program(
        mut self,
        program: super::compile_driver::NativeCodegenProgram,
        timing: bool,
        compile_start: std::time::Instant,
    ) -> Result<JitProgram, JitCompileError> {
        if let CodeOwner::ProcessMemory(owner) = &mut self.module {
            owner.machine_cache =
                MachineCodeCache::open(&self.tir_cache, CodePlacement::ProcessMemory);
        }
        let NativeCodegenProgram {
            final_functions,
            emit_resolver_here,
            app_callable_manifest,
        } = program;
        let functions = final_functions.expect("JIT requires final function custody");
        let table = molt_ir::FunctionTable::of(&functions.ir().functions);
        let (declarations, linkage) = self.declare_final_functions(&functions, &table);
        let runtime =
            crate::runtime_callable_symbols::RuntimeCallableSymbols::capture().runtime_fn_set();
        let func_count = functions.bodies().count();
        let total_ops: usize = functions
            .bodies()
            .map(crate::tir::TirFunction::operation_count)
            .sum();
        eprintln!("MOLT_BACKEND: staging {func_count} JIT entries ({total_ops} terminal ops)");
        let CodeOwner::ProcessMemory(owner) = &mut self.module else {
            panic!("JIT compilation requires a process-memory code owner");
        };
        let mut unresolved = BTreeSet::new();
        for (id, definition) in functions.into_local_functions() {
            owner.admit_explicit_imports(&definition.tir, &mut unresolved);
            Self::emit_walker_metadata(&mut owner.module, &definition.tir);
            owner
                .stage_definition(declarations[id.index()], definition)
                .map_err(JitCompileError::Finalize)?;
        }
        if !unresolved.is_empty() {
            return Err(JitCompileError::UnresolvedImports(
                unresolved.into_iter().collect(),
            ));
        }
        self.emit_cranelift_metadata(emit_resolver_here, &app_callable_manifest);
        let SimpleBackend {
            module, import_ids, ..
        } = self;
        let CodeOwner::ProcessMemory(owner) = module else {
            panic!("JIT compilation requires a process-memory code owner");
        };
        let finalize_start = std::time::Instant::now();
        let program = (*owner).finalize(
            Some(lazy::CodegenInputs {
                table,
                linkage,
                runtime,
                import_ids,
            }),
            timing,
        )?;
        if molt_ir::backend_environment::compilation_diagnostics_requested() {
            program.code.materialize_all()?;
        }
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
    fn stage_definition(
        &mut self,
        id: cranelift_module::FuncId,
        definition: NativeFinalFunction,
    ) -> Result<(), String> {
        let entry = self.code.stage(&mut self.module, id, definition)?;
        assert!(self.deferred.insert(id, entry).is_none());
        Ok(())
    }

    fn admit_explicit_imports(
        &self,
        body: &crate::tir::TirFunction,
        unresolved: &mut BTreeSet<String>,
    ) {
        use crate::tir::ops::{AttrValue, NATIVE_SYMBOL_ATTR, OpCode};
        for op in body.blocks.values().flat_map(|block| &block.ops) {
            if let Some(symbol) = crate::tir::call_targets::runtime_call_symbol(op)
                && op.runtime.is_none()
            {
                unresolved.insert(symbol.to_owned());
            }
            if matches!(op.opcode, OpCode::NativeSymbolAddr | OpCode::CallNative)
                && let Some(AttrValue::Str(symbol)) = op.attrs.get(NATIVE_SYMBOL_ATTR)
                && (self.lookup)(symbol).is_none()
            {
                unresolved.insert(symbol.clone());
            }
        }
    }

    fn finalize(
        mut self,
        inputs: Option<lazy::CodegenInputs>,
        timing: bool,
    ) -> Result<JitProgram, JitCompileError> {
        validate_imports(&self.module, &self.lookup, 0, 0)?;
        self.module
            .finalize_definitions()
            .map_err(|error| JitCompileError::Finalize(error.to_string()))?;
        self.code.finish(
            self.module,
            self.deferred,
            self.machine_cache,
            self.lookup,
            inputs,
            timing,
        );
        Ok(JitProgram { code: self.code })
    }
}

/// Validate declarations before Cranelift relocates any of their users.
fn validate_imports(
    module: &JITModule,
    lookup: &JitSymbolLookup,
    previous_functions: usize,
    previous_data: usize,
) -> Result<(), JitCompileError> {
    let declarations = module.declarations();
    let function_imports = declarations
        .get_functions()
        .skip(previous_functions)
        .filter(|(_, declaration)| declaration.linkage == Linkage::Import)
        .filter_map(|(_, declaration)| declaration.name.as_deref());
    let data_imports = declarations
        .get_data_objects()
        .skip(previous_data)
        .filter(|(_, declaration)| declaration.linkage == Linkage::Import)
        .filter_map(|(_, declaration)| declaration.name.as_deref());
    let mut unresolved: Vec<String> = function_imports
        .chain(data_imports)
        .filter(|name| *name != lazy::MATERIALIZE_SYMBOL && lookup(name).is_none())
        .map(str::to_owned)
        .collect();
    if !unresolved.is_empty() {
        unresolved.sort();
        unresolved.dedup();
        return Err(JitCompileError::UnresolvedImports(unresolved));
    }
    Ok(())
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
    let program = owner.finalize(None, false)?;
    let address = program
        .declared_function_address(PROBE_SYMBOL)
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
