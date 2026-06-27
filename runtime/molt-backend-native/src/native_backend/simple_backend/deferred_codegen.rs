use super::*;

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

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    /// Compile all deferred function definitions in parallel using rayon,
    /// then define the resulting bytes sequentially via `define_function_bytes`.
    /// Any Cranelift compile failure aborts codegen instead of producing a
    /// partial object file with runtime-aborting placeholders.
    pub(in crate::native_backend::simple_backend) fn flush_deferred_defines(&mut self) {
        use cranelift_codegen::control::ControlPlane;
        use rayon::prelude::*;

        let deferred: Vec<DeferredDefine> = std::mem::take(&mut self.deferred_defines);
        if deferred.is_empty() {
            return;
        }

        // Compile all functions in parallel. Each worker gets its own
        // Context + ControlPlane but shares one rebuilt OwnedTargetIsa.
        struct CompiledFunc {
            func_id: cranelift_module::FuncId,
            name: String,
            alignment: u64,
            code: Vec<u8>,
            relocs: Vec<cranelift_module::ModuleReloc>,
        }

        let compile_isa = Self::rebuild_owned_isa(self.module.isa(), None)
            .unwrap_or_else(|err| panic!("failed to rebuild TargetIsa for deferred flush: {err}"));

        let results: Vec<CompiledFunc> = {
            // Arc<dyn TargetIsa> contains a raw pointer that isn't marked
            // Send/Sync, but the target ISA is immutable after construction and
            // safe to share across parallel Cranelift compilation workers.
            #[derive(Clone)]
            struct SendIsa(std::sync::Arc<dyn cranelift_codegen::isa::TargetIsa>);
            unsafe impl Send for SendIsa {}
            unsafe impl Sync for SendIsa {}

            let compile_isa = SendIsa(compile_isa);
            let mut indexed: Vec<(usize, CompiledFunc)> = deferred
                .into_par_iter()
                .enumerate()
                .map(|(idx, item)| {
                    let DeferredDefine {
                        func_id,
                        func,
                        name,
                    } = item;
                    let isa = compile_isa.clone().0;
                    let mut ctx = Context::for_function(func);
                    let mut ctrl = ControlPlane::default();
                    if let Err(err) = ctx.compile(&*isa, &mut ctrl) {
                        let message = format!("Cranelift compilation failed for `{name}`: {err:?}");
                        let _ = crate::debug_artifacts::append_debug_artifact(
                            "native/cranelift_errors.txt",
                            format!("{message}\n"),
                        );
                        panic!("{message}");
                    }
                    let compiled = ctx.compiled_code().unwrap_or_else(|| {
                        panic!("Cranelift produced no compiled code for `{name}`")
                    });
                    let alignment = compiled.buffer.alignment as u64;
                    let code = compiled.buffer.data().to_vec();
                    let relocs: Vec<cranelift_module::ModuleReloc> = compiled
                        .buffer
                        .relocs()
                        .iter()
                        .map(|r| {
                            cranelift_module::ModuleReloc::from_mach_reloc(r, &ctx.func, func_id)
                        })
                        .collect();
                    (
                        idx,
                        CompiledFunc {
                            func_id,
                            name,
                            alignment,
                            code,
                            relocs,
                        },
                    )
                })
                .collect();
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
            self.defined_func_names.insert(cf.name);
        }
    }
}
