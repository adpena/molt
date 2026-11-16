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
use inkwell::module::Linkage;
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

    /// Export each compiled host entry (`runtime_abi.toml`
    /// `compiled_entries`: `molt_main`, and `molt_host_init` when the program
    /// defines it) as a bootstrap wrapper over its renamed internal body.
    ///
    /// A host calls either entry first: `run_wasm.js` calls `molt_main`, and
    /// the browser host's export invoker calls `molt_host_init` before any
    /// user export. Both wrappers run the one bootstrap, as the WASM
    /// backend's `compile_entry_wrapper` does for both.
    ///
    /// The wasm32 runtime admits a dynamic `intrinsics.require(name)` only for
    /// the names the application publishes (closed world): the app callable
    /// manifest's runtime callables, NUL-separated, handed to
    /// `molt_set_intrinsic_manifest` before the runtime initializes.
    pub fn emit_wasm_entry_wrapper(
        &self,
        has_module_registry: bool,
        intrinsic_names: &std::collections::BTreeSet<String>,
    ) {
        let manifest = intrinsic_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\0");
        let manifest_bytes = self.context.const_string(manifest.as_bytes(), false);
        let manifest_global =
            self.module
                .add_global(manifest_bytes.get_type(), None, "__molt_intrinsic_manifest");
        manifest_global.set_linkage(Linkage::Private);
        manifest_global.set_constant(true);
        manifest_global.set_initializer(&manifest_bytes);
        let manifest = (manifest_global, manifest.len() as u64);
        self.wrap_wasm_entry(
            molt_ir::PROGRAM_ENTRY_SYMBOL,
            true,
            has_module_registry,
            manifest,
        );
        self.wrap_wasm_entry(
            molt_ir::HOST_INIT_SYMBOL,
            false,
            has_module_registry,
            manifest,
        );
    }

    fn wrap_wasm_entry(
        &self,
        entry: &str,
        required: bool,
        has_module_registry: bool,
        (manifest_global, manifest_len): (inkwell::values::GlobalValue<'ctx>, u64),
    ) {
        let ctx = self.context;
        let module = &self.module;
        let symbol = compiled_execution_symbol(entry);
        let Some(body) = module
            .get_function(&symbol)
            .filter(|body| body.count_basic_blocks() > 0)
        else {
            assert!(!required, "LLVM WASM entry: `{symbol}` is not defined");
            return;
        };
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

        // Every host calls `entry(context) -> word` (runtime_abi.toml
        // `compiled_entries`): the wrapper returns one boxed word whatever the
        // body returns.
        let params = body.get_type().get_param_types();
        let wrapper = module.add_function(
            &symbol,
            i64_ty.fn_type(&params, false),
            Some(Linkage::External),
        );
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
        let set_manifest = self
            .runtime_imports
            .get(ctx, module, RuntimeFn::SetIntrinsicManifest);
        let manifest_ptr = builder
            .build_ptr_to_int(
                manifest_global.as_pointer_value(),
                i64_ty,
                "intrinsic_manifest",
            )
            .unwrap();
        builder
            .build_call(
                set_manifest,
                &[
                    manifest_ptr.into(),
                    i64_ty.const_int(manifest_len, false).into(),
                ],
                "",
            )
            .unwrap();
        builder.build_call(runtime_init, &[], "").unwrap();
        let args: Vec<inkwell::values::BasicMetadataValueEnum> =
            wrapper.get_param_iter().map(Into::into).collect();
        let result = builder.build_call(body, &args, "entry").unwrap();
        let word = match body.get_type().get_return_type() {
            Some(inkwell::types::BasicTypeEnum::IntType(ty)) if ty.get_bit_width() == 64 => {
                result.try_as_basic_value().unwrap_basic().into_int_value()
            }
            Some(other) => panic!("LLVM WASM entry: `{entry}` body returns {other:?}"),
            None => i64_ty.const_int(molt_codegen_abi::box_none_bits() as u64, false),
        };
        builder.build_return(Some(&word)).unwrap();
    }
}
