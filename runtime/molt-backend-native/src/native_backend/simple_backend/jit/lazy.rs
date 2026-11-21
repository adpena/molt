//! Stable compiled entry points and synchronized first-call machine code.

use super::super::deferred_codegen::{
    DeferredDefine, assemble_definition, record_compiled_function,
};
use super::*;
use cranelift_module::FuncId;
use molt_ir::backend_environment::{self, Carried};
use molt_ir::linkage_abi::NativeFunctionLinkageAbi;
use molt_ir::runtime_callable_symbols::RuntimeFnSet;
use std::io::Write;
use std::sync::OnceLock;

pub(super) const MATERIALIZE_SYMBOL: &str = "__molt_jit_materialize";

pub(super) fn materializer_address() -> *const u8 {
    materialize as *const u8
}

#[derive(Default)]
pub(super) struct LazyCode {
    state: OnceLock<Mutex<ReadyCode>>,
}

struct ReadyCode {
    module: JITModule,
    deferred: BTreeMap<FuncId, Entry>,
    cache: Option<MachineCodeCache>,
    lookup: JitSymbolLookup,
    inputs: Option<CodegenInputs>,
    request: Carried,
    timing: bool,
}

pub(super) struct CodegenInputs {
    pub(super) table: molt_ir::FunctionTable,
    pub(super) linkage: BTreeMap<String, NativeFunctionLinkageAbi>,
    pub(super) runtime: RuntimeFnSet,
    pub(super) import_ids: RuntimeImportIds,
}

pub(super) struct Entry {
    address: Box<AtomicUsize>,
    pending: Option<(FuncId, NativeFinalFunction)>,
}

impl LazyCode {
    pub(super) fn finish(
        &self,
        module: JITModule,
        deferred: BTreeMap<FuncId, Entry>,
        cache: Option<MachineCodeCache>,
        lookup: JitSymbolLookup,
        inputs: Option<CodegenInputs>,
        timing: bool,
    ) {
        let state = ReadyCode {
            module,
            deferred,
            cache,
            lookup,
            inputs,
            request: backend_environment::carry(),
            timing,
        };
        assert!(self.state.set(Mutex::new(state)).is_ok());
    }

    pub(super) fn with_module<T>(&self, read: impl FnOnce(&JITModule) -> T) -> T {
        read(&self.state.get().unwrap().lock().unwrap().module)
    }

    pub(super) fn stage(
        &self,
        module: &mut JITModule,
        dispatch: FuncId,
        definition: NativeFinalFunction,
    ) -> Result<Entry, String> {
        let signature = module
            .declarations()
            .get_function_decl(dispatch)
            .signature
            .clone();
        let body = module
            .declare_anonymous_function(&signature)
            .map_err(|err| err.to_string())?;
        let address = Box::new(AtomicUsize::new(0));
        let mut thunk = module.make_context();
        thunk.func.signature = signature.clone();
        let pointer = module.target_config().pointer_type();
        let mut resolver_signature = module.make_signature();
        resolver_signature
            .params
            .extend([AbiParam::new(pointer), AbiParam::new(types::I32)]);
        resolver_signature.returns.push(AbiParam::new(pointer));
        let resolver = module
            .declare_function(MATERIALIZE_SYMBOL, Linkage::Import, &resolver_signature)
            .map_err(|err| err.to_string())?;

        let mut context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut thunk.func, &mut context);
        let entry = builder.create_block();
        let missing = builder.create_block();
        let invoke = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.append_block_param(invoke, pointer);
        builder.switch_to_block(entry);
        let parameters = builder.block_params(entry).to_vec();
        let slot = builder
            .ins()
            .iconst(pointer, address.as_ref() as *const AtomicUsize as i64);
        let published = builder
            .ins()
            .atomic_load(pointer, MemFlagsData::trusted(), slot);
        builder
            .ins()
            .brif(published, invoke, &[published.into()], missing, &[]);

        builder.switch_to_block(missing);
        let resolver_ref = module.declare_func_in_func(resolver, builder.func);
        let state = builder.ins().iconst(pointer, self as *const Self as i64);
        let id = builder
            .ins()
            .iconst(types::I32, i64::from(dispatch.as_u32()));
        let call = builder.ins().call(resolver_ref, &[state, id]);
        let published = builder.inst_results(call)[0];
        builder.ins().jump(invoke, &[published.into()]);

        builder.switch_to_block(invoke);
        let address_value = builder.block_params(invoke)[0];
        let callable = builder.import_signature(signature);
        let call = builder
            .ins()
            .call_indirect(callable, address_value, &parameters);
        let returns = builder.inst_results(call).to_vec();
        builder.ins().return_(&returns);
        builder.seal_all_blocks();
        builder.finalize(module.target_config());
        module
            .define_function(dispatch, &mut thunk)
            .map_err(|err| err.to_string())?;
        Ok(Entry {
            address,
            pending: Some((body, definition)),
        })
    }

    pub(super) fn materialize_all(&self) -> Result<(), JitCompileError> {
        let mut code = self.state.get().unwrap().lock().unwrap();
        let declarations: Vec<_> = code.deferred.keys().copied().collect();
        for declaration in declarations {
            code.materialize(declaration)?;
        }
        Ok(())
    }
}

extern "C" fn materialize(state: *const LazyCode, ordinal: u32) -> usize {
    // SAFETY: The generated thunk embeds the address of the program's boxed
    // owner. No thunk is callable before the owner is finalized or after it
    // is dropped. The published code and its slot share that same lifetime.
    let owner = unsafe { &*state };
    let result = owner
        .state
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .materialize(FuncId::from_u32(ordinal));
    match result {
        Ok(address) => address,
        Err(error) => {
            // A guest may have closed the diagnostic pipe or limited file
            // writes. Deliver the runner status even if the report cannot be
            // written. This calling thread exits without executing guest code.
            #[cfg(unix)]
            unsafe {
                let mut signals = std::mem::zeroed::<libc::sigset_t>();
                libc::sigemptyset(&mut signals);
                libc::sigaddset(&mut signals, libc::SIGPIPE);
                libc::sigaddset(&mut signals, libc::SIGXFSZ);
                libc::pthread_sigmask(libc::SIG_BLOCK, &signals, std::ptr::null_mut());
            }
            let _ = writeln!(
                std::io::stderr().lock(),
                "molt-jit: first-call compilation: {error}"
            );
            std::process::exit(JIT_FAILURE_STATUS);
        }
    }
}

impl ReadyCode {
    fn materialize(&mut self, id: FuncId) -> Result<usize, JitCompileError> {
        let Self {
            module,
            deferred,
            cache,
            lookup,
            inputs,
            request,
            timing,
        } = self;
        let entry = deferred
            .get_mut(&id)
            .expect("the stub names its declared body");
        let current = entry.address.load(Ordering::Acquire);
        if current != 0 {
            return Ok(current);
        }
        let (body_id, definition) = entry.pending.take().expect("unpublished body");
        let CodegenInputs {
            table,
            linkage,
            runtime,
            import_ids,
        } = inputs.as_mut().expect("staged bodies have codegen inputs");
        let previous_functions = module.declarations().get_functions().count();
        let previous_data = module.declarations().get_data_objects().count();
        let cached = cache.as_ref().filter(|_| cache_io_allowed());
        let compiled = request.enter(|| {
            let func = SimpleBackend::lower_cranelift_definition(
                module,
                import_ids,
                body_id,
                &definition,
                table,
                linkage,
                runtime,
            )
            .map_err(JitCompileError::Lowering)?;
            let name = definition.tir.name;
            record_compiled_function(&name, &func);
            validate_imports(module, lookup, previous_functions, previous_data)?;
            assemble_definition(
                DeferredDefine {
                    func_id: body_id,
                    func,
                    name,
                },
                module.isa(),
                cached,
            )
            .map_err(|error| JitCompileError::Lowering(vec![error]))
        })?;
        let finalize_error =
            |error| JitCompileError::Finalize(format!("`{}`: {error}", compiled.name));
        module
            .define_function_bytes(
                body_id,
                compiled.alignment,
                &compiled.code,
                &compiled.relocs,
            )
            .map_err(finalize_error)?;
        module.finalize_definitions().map_err(finalize_error)?;
        let address = module.get_finalized_function(body_id) as usize;
        // Publish after code and its relocations are executable on all cores.
        entry.address.store(address, Ordering::Release);
        if *timing {
            let _ = writeln!(
                std::io::stderr().lock(),
                "MOLT_BACKEND_TIMING: JIT materialized {}",
                compiled.name
            );
            if let Some(cache) = cache {
                cache.report_counts();
            }
        }
        Ok(address)
    }
}

/// During Python execution a finite RLIMIT_FSIZE can deliver SIGXFSZ on a
/// write. The cache can write on admission when it quarantines a corrupt entry.
/// Compile without cache I/O when the guest sets a finite limit.
#[cfg(unix)]
fn cache_io_allowed() -> bool {
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: getrlimit writes the whole struct on success.
    unsafe {
        libc::getrlimit(libc::RLIMIT_FSIZE, limit.as_mut_ptr()) == 0
            && limit.assume_init().rlim_cur == libc::RLIM_INFINITY
    }
}

#[cfg(not(unix))]
fn cache_io_allowed() -> bool {
    true
}
