use super::*;
use molt_ir::backend_environment::{self as backend_env, BackendEnv};

pub(super) fn debug_file_stem(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(in crate::native_backend) fn inspect_native_cfg(name: &str, function: &Function) {
    // Zero-predecessor blocks are harmless dead code that Cranelift
    // skips during compilation.  Only log them when debugging.
    if backend_env::var_os(BackendEnv::MOLT_DUMP_CLIF_ON_CFG_ERROR).is_some() {
        let zero_pred_blocks = find_zero_pred_blocks(function);
        if !zero_pred_blocks.is_empty() {
            eprintln!(
                "Backend CFG issue in {}: zero-predecessor blocks {:?}",
                name, zero_pred_blocks
            );
            eprintln!("CLIF {}:\n{}", name, function.display());
        }
    }
    if let Ok(filter) = backend_env::var(BackendEnv::MOLT_DUMP_CLIF_FUNC)
        && (name == filter || name.contains(&filter))
    {
        eprintln!("CLIF {}:\n{}", name, function.display());
    }
    if let Ok(path) = backend_env::var(BackendEnv::MOLT_DUMP_CLIF_FILE)
        && let Ok(clif_filter) = backend_env::var(BackendEnv::MOLT_DUMP_CLIF_FILE_FILTER)
        && name.contains(&clif_filter)
    {
        let clif_text = format!("CLIF {}:\n{}", name, function.display());
        let _ = std::fs::write(&path, &clif_text);
    }
}

pub(super) fn record_compiled_function(name: &str, function: &Function) {
    if backend_env::var(BackendEnv::MOLT_DEBUG_COMPILED_FUNCS).as_deref() == Ok("1") {
        let _ = crate::debug_artifacts::append_debug_artifact(
            "native/compiled_funcs.txt",
            format!("compiled: {}\n", name),
        );
    }
    if let Ok(filter) = backend_env::var(BackendEnv::MOLT_DUMP_CLIF)
        && (filter == "1" || filter == name || name.contains(&filter))
    {
        let clif = format!("{}", function.display());
        eprintln!("CLIF {}:\n{}", name, clif);
        let sanitized = debug_file_stem(name);
        let _ = crate::debug_artifacts::write_debug_artifact(format!("clif/{sanitized}.txt"), clif);
    }
}

#[cfg(feature = "native-backend")]
pub(crate) const DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT: usize = 16;
#[cfg(feature = "native-backend")]
pub(crate) const DEFERRED_CODEGEN_FLUSH_OP_BUDGET: usize = 4_000;

#[cfg(feature = "native-backend")]
pub(crate) fn should_flush_deferred_codegen(deferred_count: usize, deferred_ops: usize) -> bool {
    deferred_count > 0
        && (deferred_count >= DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT
            || deferred_ops >= DEFERRED_CODEGEN_FLUSH_OP_BUDGET)
}

#[cfg(feature = "native-backend")]
pub(crate) struct DeferredDefine {
    pub(crate) func_id: cranelift_module::FuncId,
    pub(crate) func: cranelift_codegen::ir::Function,
    pub(crate) name: String,
}

pub(super) struct CompiledDefinition {
    pub(super) func_id: cranelift_module::FuncId,
    pub(super) name: String,
    pub(super) alignment: u64,
    pub(super) code: Vec<u8>,
    pub(super) relocs: Vec<cranelift_module::ModuleReloc>,
}

pub(super) fn assemble_definition(
    definition: DeferredDefine,
    isa: &dyn isa::TargetIsa,
    #[cfg(feature = "jit")] cache: Option<&super::jit::MachineCodeCache>,
) -> Result<CompiledDefinition, String> {
    let DeferredDefine {
        func_id,
        func,
        name,
    } = definition;
    let mut context = Context::for_function(func);
    complete_native_control_flow(&mut context, isa, &name);
    let mut control = cranelift_codegen::control::ControlPlane::default();
    #[cfg(feature = "jit")]
    let compile = match cache {
        Some(cache) => cache.compile(&mut context, isa, &mut control),
        None => context.compile(isa, &mut control),
    };
    #[cfg(not(feature = "jit"))]
    let compile = context.compile(isa, &mut control);
    compile.map_err(|error| format!("Cranelift compilation failed for `{name}`: {error:?}"))?;
    let compiled = context
        .compiled_code()
        .expect("successful Cranelift compilation");
    let alignment = compiled.buffer.min_alignment as u64;
    let code = compiled.buffer.data().to_vec();
    let relocs = compiled
        .buffer
        .relocs()
        .iter()
        .map(|reloc| cranelift_module::ModuleReloc::from_mach_reloc(reloc, &context.func, func_id))
        .collect();
    Ok(CompiledDefinition {
        func_id,
        name,
        alignment,
        code,
        relocs,
    })
}

/// Complete actual CFG ownership after FunctionBuilder has released SSA custody.
/// Emission-time reachability cannot authorize keeping a dead block or inventing
/// a terminator. Cranelift retires the whole unreachable component and its tables.
#[cfg(feature = "native-backend")]
pub(super) fn complete_native_control_flow(
    context: &mut Context,
    isa: &dyn isa::TargetIsa,
    name: &str,
) {
    context.flowgraph();
    for block in context.func.layout.blocks() {
        assert!(
            !context.domtree.is_reachable(block) || context.func.layout.last_inst(block).is_some(),
            "native codegen left REACHABLE block {block:?} empty (no terminator) in '{name}'; \
             fix its producer instead of inventing a trap or fallthrough"
        );
    }
    context
        .eliminate_unreachable_code(isa)
        .unwrap_or_else(|error| panic!("native CFG completion failed for '{name}': {error:?}"));
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    /// Both code owners publish a body through the same deferred verifier and compiler.
    pub(in crate::native_backend) fn queue_compiled_definition(
        &mut self,
        id: cranelift_module::FuncId,
        name: &str,
        function: Function,
    ) {
        record_compiled_function(name, &function);
        self.deferred_defines.push(DeferredDefine {
            func_id: id,
            func: function,
            name: name.to_owned(),
        });
    }

    /// Compile all deferred function definitions in parallel using rayon,
    /// then define the resulting bytes sequentially via `define_function_bytes`.
    /// Any Cranelift compile failure aborts codegen instead of producing a
    /// partial object file with runtime-aborting placeholders.
    pub(in crate::native_backend::simple_backend) fn flush_deferred_defines(&mut self) {
        use rayon::prelude::*;

        let deferred: Vec<DeferredDefine> = std::mem::take(&mut self.deferred_defines);
        if deferred.is_empty() {
            return;
        }

        // The AOT workers share an immutable ISA and define the bytes in
        // source order after parallel machine-code compilation.
        let compile_isa = Self::rebuild_owned_isa(self.module.isa(), None)
            .unwrap_or_else(|err| panic!("failed to rebuild TargetIsa for deferred flush: {err}"));

        let results: Vec<CompiledDefinition> = {
            // Arc<dyn TargetIsa> contains a raw pointer that isn't marked
            // Send/Sync, but the target ISA is immutable after construction and
            // safe to share across parallel Cranelift compilation workers.
            #[derive(Clone)]
            struct SendIsa(std::sync::Arc<dyn cranelift_codegen::isa::TargetIsa>);
            unsafe impl Send for SendIsa {}
            unsafe impl Sync for SendIsa {}

            let compile_isa = SendIsa(compile_isa);
            let request = crate::backend_environment::carry();
            // The request's pool honors its CPU limit and keeps concurrent
            // compiles within one shared set of threads.
            let pool = molt_tir::tir::pipeline_cache::request_thread_pool();
            let mut indexed: Vec<(usize, CompiledDefinition)> = pool.install(|| {
                deferred
                    .into_par_iter()
                    .enumerate()
                    .map(|(idx, item)| {
                        request.enter(|| {
                            let isa = compile_isa.clone().0;
                            let result = assemble_definition(
                                item,
                                &*isa,
                                #[cfg(feature = "jit")]
                                None,
                            )
                            .unwrap_or_else(|message| {
                                let _ = crate::debug_artifacts::append_debug_artifact(
                                    "native/cranelift_errors.txt",
                                    format!("{message}\n"),
                                );
                                panic!("{message}");
                            });
                            (idx, result)
                        })
                    })
                    .collect()
            });
            indexed.sort_by_key(|(idx, _)| *idx);
            indexed.into_iter().map(|(_, result)| result).collect()
        };

        // Sequential phase: define compiled functions in original order.
        for cf in results {
            self.module
                .define_function_bytes(cf.func_id, cf.alignment, &cf.code, &cf.relocs)
                .unwrap_or_else(|err| {
                    panic!("define_function_bytes failed for `{}`: {err}", cf.name)
                });
            self.defined_func_names
                .insert(compiled_execution_symbol(&cf.name));
        }
    }
}
