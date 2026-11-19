//! LLVM instruction selection for compiled method sites (call_anatomy.md 4.5
//! "Methods"; `molt_codegen_abi::method_site` owns the meaning). Each site's
//! record is a writable internal global holding the generated record image.
//! As the frame and slot helpers are, the load and each call arity get one
//! always-inlined internal body, so their explicit blocks never split the
//! caller's TIR block and its phi edges stay intact.
use super::frame_record::LlvmFrameBuilder;
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use inkwell::values::IntValue;
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::method_site::{
    MethodSiteBuilder, generate_method_site_call, generate_method_site_load,
};

impl<'ctx> MethodSiteBuilder for LlvmFrameBuilder<'_, 'ctx> {
    fn call_entry(
        &mut self,
        entry: IntValue<'ctx>,
        self_word: IntValue<'ctx>,
        args: &[IntValue<'ctx>],
        defaults: &[IntValue<'ctx>],
    ) -> IntValue<'ctx> {
        let context = self.words.context;
        let i64_type = context.i64_type();
        let words: Vec<_> = std::iter::once(self_word)
            .chain(args.iter().copied())
            .chain(defaults.iter().copied())
            .collect();
        // The fixed-arity entry: the execution context, then boxed words.
        let params = vec![i64_type.into(); words.len() + 1];
        let signature = i64_type.fn_type(&params, false);
        let execution_context = self
            .function
            .get_first_param()
            .expect("a compiled helper takes the execution context")
            .into_int_value();
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            vec![execution_context.into()];
        call_args.extend(
            words
                .iter()
                .map(|&word| inkwell::values::BasicMetadataValueEnum::from(word)),
        );
        let target = self
            .words
            .builder
            .build_int_to_ptr(
                entry,
                context.ptr_type(inkwell::AddressSpace::default()),
                "method_entry",
            )
            .unwrap();
        self.words
            .builder
            .build_indirect_call(signature, target, &call_args, "method_call")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }
}

impl<'ctx> LlvmBackend<'ctx> {
    /// The method site call body with `nargs` arguments
    /// (`SiteBody::MethodSiteCall`): words `[thread, record, callable, self,
    /// args..., admission]`, whose miss is `cold`.
    pub(super) fn method_site_call_helper(
        &self,
        nargs: usize,
        cold: RuntimeFn,
    ) -> FunctionValue<'ctx> {
        let cold = self.runtime_import(cold);
        self.site_helper(
            &format!("__molt_method_site_call_{nargs}"),
            4 + nargs + 1,
            |b, words| {
                let (thread, record, callable, self_word) =
                    (words[0], words[1], words[2], words[3]);
                let args = &words[4..];
                generate_method_site_call(b, thread, record, callable, self_word, args, |b| {
                    let cold_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                        words[2..].iter().map(|&word| word.into()).collect();
                    b.words
                        .builder
                        .build_execution_call(cold, &cold_args, "method_site_call_miss")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                        .into_int_value()
                })
            },
        )
    }
}

impl<'ctx> LlvmBackend<'ctx> {
    /// The method site load body (`SiteBody::MethodSiteLoad`): words
    /// `[record, receiver, self_out, admission]`, whose miss is
    /// `molt_method_site_load(record, receiver, self_out)`.
    pub(super) fn method_site_load_helper(&self) -> FunctionValue<'ctx> {
        let cold = self.runtime_import(RuntimeFn::MethodSiteLoad);
        self.site_helper("__molt_method_site_load", 4, |b, words| {
            let &[record, receiver, self_out] = words else {
                unreachable!("the load helper's parameters")
            };
            generate_method_site_load(b, record, receiver, self_out, |b| {
                b.words
                    .builder
                    .build_execution_call(
                        cold,
                        &[record.into(), receiver.into(), self_out.into()],
                        "method_site_load_miss",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value()
            })
        })
    }

    /// The program's private, writable data object `symbol`, created with
    /// `image` on first request (`CodegenBuilder::site_record`).
    pub(super) fn site_record(
        &self,
        symbol: &str,
        image: &[u8],
    ) -> inkwell::values::GlobalValue<'ctx> {
        if let Some(global) = self.module.get_global(symbol) {
            return global;
        }
        let context = self.context;
        let ty = context.i8_type().array_type(image.len() as u32);
        let global = self.module.add_global(ty, None, symbol);
        global.set_linkage(inkwell::module::Linkage::Internal);
        global.set_initializer(&context.const_string(image, false));
        global.set_constant(false);
        global.set_alignment(8);
        global
    }

    /// An always-inlined helper `name` taking `params` words and returning
    /// one, with its body written by `body` on a fresh frame builder.
    pub(super) fn site_helper(
        &self,
        name: &str,
        params: usize,
        body: impl FnOnce(&mut LlvmFrameBuilder<'_, 'ctx>, &[IntValue<'ctx>]) -> IntValue<'ctx>,
    ) -> FunctionValue<'ctx> {
        let context = self.context;
        let i64_type = context.i64_type();
        let logical_signature = i64_type.fn_type(&vec![i64_type.into(); params], false);
        let signature = prepend_execution_context(logical_signature);
        if let Some(function) = self.module.get_compiled_function(name) {
            return require_llvm_function_type(name, function, signature);
        }
        let function = self.module.add_compiled_function(
            name,
            logical_signature,
            Some(inkwell::module::Linkage::Internal),
        );
        function.add_attribute(
            AttributeLoc::Function,
            context.create_enum_attribute(
                inkwell::attributes::Attribute::get_named_enum_kind_id("alwaysinline"),
                0,
            ),
        );
        let entry = context.append_basic_block(function, "entry");
        let body_block = context.append_basic_block(function, "body");
        let builder = context.create_builder();
        builder.position_at_end(body_block);
        // Parameter 0 is the execution context; the last is the RC admission.
        let words: Vec<IntValue<'ctx>> = (1..=params as u32)
            .map(|index| function.get_nth_param(index).unwrap().into_int_value())
            .collect();
        let admission = *words.last().expect("a site helper takes the RC admission");
        let mut sites = LlvmFrameBuilder {
            words: LlvmWords {
                context,
                builder: &builder,
            },
            function,
            entry,
            admission,
            retain: self.ensure_refcount_helper(RefcountOperation::Retain),
            release: self.ensure_refcount_helper(RefcountOperation::Release),
            enter: self.runtime_import(RuntimeFn::FrameRecordEnter),
            exit: self.runtime_import(RuntimeFn::FrameRecordExit),
            slow_blocks: Vec::new(),
            task_imports: None,
        };
        let result = body(&mut sites, &words[..params - 1]);
        builder.build_return(Some(&result)).unwrap();
        // The locals were allocated in `entry` as the body asked for them.
        builder.position_at_end(entry);
        builder.build_unconditional_branch(body_block).unwrap();
        function
    }

    /// This module's declaration of runtime entry `function`.
    pub(super) fn runtime_import(&self, function: RuntimeFn) -> FunctionValue<'ctx> {
        self.runtime_imports
            .get(self.context, &self.module, function)
    }
}
