//! The WASM application entry: the one exported `molt_main` a host calls.
//!
//! A native program starts in the CLI's C main stub, which registers the app
//! resolver and the module registry, initializes the runtime and calls the
//! compiled `molt_main`. A WASM module has no C stub: the host (Node's
//! `run_wasm.js`, Wasmtime's `molt-wasm-host`) calls the exported
//! `molt_main` with the attached execution context. That export is this
//! wrapper. It runs the same bootstrap the WASM backend's entry wrapper runs
//! (`molt-backend-wasm` `emit_host_init_sequence`), then the compiled body.

#[cfg(feature = "llvm")]
use super::LlvmBackend;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::{ExecutionBuilderExt, ExecutionModuleExt};
#[cfg(feature = "llvm")]
use inkwell::{AddressSpace, module::Linkage};
#[cfg(feature = "llvm")]
use molt_ir::runtime_callable_abi_generated::compiled_execution_symbol;
#[cfg(feature = "llvm")]
use molt_ir::runtime_fn_generated::RuntimeFn;

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    /// The app half of the wasm32 module-import contract: the runtime calls
    /// `molt_isolate_import(context, module_id)` (molt-runtime
    /// `module_table.rs`) and this switch calls that module's initializer, as
    /// the WASM backend's `emit_module_registry_dispatch` does. A missing id
    /// returns None.
    pub fn emit_wasm_isolate_import(&self, registry: &molt_ir::ModuleRegistryIR) {
        let ctx = self.context;
        let module = &self.module;
        let i64_ty = ctx.i64_type();
        let dispatcher = module.add_compiled_function(
            molt_ir::ISOLATE_IMPORT_SYMBOL,
            i64_ty.fn_type(&[i64_ty.into()], false),
            Some(Linkage::External),
        );
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(dispatcher, "entry");
        let missing = ctx.append_basic_block(dispatcher, "missing");
        let none = i64_ty.const_int(molt_codegen_abi::box_none_bits() as u64, false);
        let mut cases = Vec::new();
        for (id, symbol) in registry.init_rows() {
            // A batch or the shared stdlib archive may define it; wasm-ld
            // resolves the declaration, as for the registry blob's column.
            let init = crate::llvm_backend::lowering::linked_compiled_function(self, symbol);
            let block = ctx.append_basic_block(dispatcher, "init");
            builder.position_at_end(block);
            builder.build_execution_call(init, &[], "").unwrap();
            builder.build_return(Some(&none)).unwrap();
            cases.push((i64_ty.const_int(u64::from(id), false), block));
        }
        builder.position_at_end(missing);
        builder.build_return(Some(&none)).unwrap();
        builder.position_at_end(entry);
        let module_id = dispatcher.get_nth_param(1).unwrap().into_int_value();
        builder.build_switch(module_id, missing, &cases).unwrap();
    }

    /// The app half of the wasm32 indirect-call contract: the runtime calls a
    /// compiled function through `molt_call_indirectN(context, slot, args..)`
    /// (molt-runtime `lib.rs`), as the WASM backend's
    /// `emit_call_indirect_exports_and_sentinel` defines it. A slot above
    /// `u32::MAX` is a runtime key, never a table index, and traps (HF-699 K2).
    /// The arities are the generated execution ABI's `molt_call_indirectN`.
    pub fn emit_wasm_call_indirect_trampolines(&self) {
        let ctx = self.context;
        let module = &self.module;
        let i64_ty = ctx.i64_type();
        let ptr_ty = ctx.ptr_type(AddressSpace::default());
        let builder = ctx.create_builder();
        let trap = module.get_function("llvm.trap").unwrap_or_else(|| {
            module.add_function("llvm.trap", ctx.void_type().fn_type(&[], false), None)
        });
        for arity in 0usize.. {
            let name = format!("molt_call_indirect{arity}");
            if molt_ir::runtime_callable_abi_generated::runtime_execution_abi(&name).is_none() {
                break;
            }
            let logical: Vec<inkwell::types::BasicMetadataTypeEnum> =
                vec![i64_ty.into(); arity + 1];
            let trampoline = module.add_compiled_function(
                &name,
                i64_ty.fn_type(&logical, false),
                Some(Linkage::External),
            );
            let entry = ctx.append_basic_block(trampoline, "entry");
            let call = ctx.append_basic_block(trampoline, "call");
            let reject = ctx.append_basic_block(trampoline, "reject");
            builder.position_at_end(entry);
            let slot = trampoline.get_nth_param(1).unwrap().into_int_value();
            let is_key = builder
                .build_int_compare(
                    inkwell::IntPredicate::UGT,
                    slot,
                    i64_ty.const_int(u64::from(u32::MAX), false),
                    "is_runtime_key",
                )
                .unwrap();
            builder
                .build_conditional_branch(is_key, reject, call)
                .unwrap();
            builder.position_at_end(reject);
            builder.build_call(trap, &[], "").unwrap();
            builder.build_unreachable().unwrap();
            builder.position_at_end(call);
            let target = builder.build_int_to_ptr(slot, ptr_ty, "target").unwrap();
            let mut args: Vec<inkwell::values::BasicMetadataValueEnum> =
                vec![trampoline.get_nth_param(0).unwrap().into()];
            for index in 0..arity {
                let arg: inkwell::values::BasicMetadataValueEnum =
                    trampoline.get_nth_param(index as u32 + 2).unwrap().into();
                args.push(arg);
            }
            let callee_ty = i64_ty.fn_type(&logical, false);
            let result = builder
                .build_indirect_call(callee_ty, target, &args, "result")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            builder.build_return(Some(&result)).unwrap();
        }
    }

    /// Record the attached execution-context ABI witness as the WASM custom
    /// section hosts check before they pass a context (`molt-codegen-abi`
    /// `ATTACHED_EXECUTION_CONTEXT_ABI_SECTION`, little-endian fingerprint).
    /// LLVM lowers `!wasm.custom_sections` to a custom section of the object,
    /// and wasm-ld carries it into the linked module, as rustc's
    /// `#[link_section]` on wasm does.
    pub fn emit_wasm_attached_context_abi_section(&self) {
        use inkwell::llvm_sys::core::{
            LLVMAddNamedMetadataOperand, LLVMMDNodeInContext2, LLVMMDStringInContext2,
            LLVMMetadataAsValue,
        };
        let name = molt_ir::runtime_callable_abi_generated::ATTACHED_EXECUTION_CONTEXT_ABI_SECTION;
        let payload =
            molt_ir::runtime_callable_abi_generated::ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT
                .to_le_bytes();
        let context = self.context.raw();
        // SAFETY: every reference belongs to this backend's live context and
        // module; LLVM copies the string bytes it is given.
        unsafe {
            let mut operands = [
                LLVMMDStringInContext2(context, name.as_ptr().cast(), name.len()),
                LLVMMDStringInContext2(context, payload.as_ptr().cast(), payload.len()),
            ];
            let node = LLVMMDNodeInContext2(context, operands.as_mut_ptr(), operands.len());
            LLVMAddNamedMetadataOperand(
                self.module.as_mut_ptr(),
                c"wasm.custom_sections".as_ptr(),
                LLVMMetadataAsValue(context, node),
            );
        }
    }

    /// Rename the compiled `molt_main` to an internal body and export the
    /// bootstrap wrapper under the physical `molt_main` symbol.
    pub fn emit_wasm_entry_wrapper(&self, has_module_registry: bool) {
        let ctx = self.context;
        let module = &self.module;
        let main_symbol = compiled_execution_symbol(molt_ir::PROGRAM_ENTRY_SYMBOL);
        let body = module
            .get_function(&main_symbol)
            .unwrap_or_else(|| panic!("LLVM WASM entry: `{main_symbol}` is not defined"));
        // The body keeps no linker identity: the wrapper is its one caller.
        body.as_global_value().set_name("");
        body.set_linkage(Linkage::Internal);

        let i64_ty = ctx.i64_type();
        let set_resolver = self
            .runtime_imports
            .get(ctx, module, RuntimeFn::SetAppCallableResolver);
        let runtime_init = self
            .runtime_imports
            .get(ctx, module, RuntimeFn::RuntimeInit);

        let wrapper = module.add_function(&main_symbol, body.get_type(), Some(Linkage::External));
        let builder = ctx.create_builder();
        builder.position_at_end(ctx.append_basic_block(wrapper, "entry"));
        let resolver = module
            .get_function(crate::app_resolver_abi::APP_RESOLVER_SYMBOL)
            .expect("LLVM WASM entry needs the app callable resolver");
        let resolver_bits = builder
            .build_ptr_to_int(
                resolver.as_global_value().as_pointer_value(),
                i64_ty,
                "resolver_bits",
            )
            .unwrap();
        builder
            .build_call(set_resolver, &[resolver_bits.into()], "")
            .unwrap();
        if has_module_registry {
            let blob = module
                .get_global(super::module_registry::MODULE_REGISTRY_BLOB_SYMBOL)
                .expect("LLVM WASM entry needs the module registry blob");
            let registry_install =
                self.runtime_imports
                    .get(ctx, module, RuntimeFn::ModuleRegistryInstall);
            builder
                .build_call(registry_install, &[blob.as_pointer_value().into()], "")
                .unwrap();
        }
        builder.build_call(runtime_init, &[], "").unwrap();
        let args: Vec<inkwell::values::BasicMetadataValueEnum> =
            wrapper.get_param_iter().map(Into::into).collect();
        let result = builder.build_call(body, &args, "main").unwrap();
        if body.get_type().get_return_type().is_some() {
            let value = result.try_as_basic_value().unwrap_basic();
            builder.build_return(Some(&value)).unwrap();
        } else {
            builder.build_return(None).unwrap();
        }
    }
}
