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
    method_site_record_size, write_method_site_record_image,
};

/// The runtime's fixed-arity loaded-method calls, the cold path of a site
/// call with that many arguments. A call with more keeps the runtime call.
const COLD_CALLS: [RuntimeFn; 5] = [
    RuntimeFn::CallLoadedMethod0,
    RuntimeFn::CallLoadedMethod1,
    RuntimeFn::CallLoadedMethod2,
    RuntimeFn::CallLoadedMethod3,
    RuntimeFn::CallLoadedMethod4,
];

impl<'ctx> MethodSiteBuilder for LlvmFrameBuilder<'_, 'ctx> {
    fn recursion_enter(&mut self) -> IntValue<'ctx> {
        let (enter, _) = self.recursion.expect("a method site call body");
        self.words
            .builder
            .build_execution_call(enter, &[], "recursion_admitted")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }

    fn recursion_exit(&mut self) {
        let (_, exit) = self.recursion.expect("a method site call body");
        self.words
            .builder
            .build_execution_call(exit, &[], "")
            .unwrap();
    }

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

impl<'ctx> FunctionLowering<'ctx, '_> {
    /// The address of method site `site`'s record in this function, as a
    /// word. The record is created with the image of the site's load.
    fn method_site_record(&mut self, site: u32) -> Option<IntValue<'ctx>> {
        let symbol = format!("__molt_method_site_{}_{site}", self.func.name);
        let global = match self.backend.module.get_global(&symbol) {
            Some(global) => global,
            None => {
                let name = self
                    .func
                    .blocks
                    .values()
                    .flat_map(|block| &block.ops)
                    .find_map(|op| match op.attrs.get("name") {
                        Some(AttrValue::Str(name))
                            if op.opcode == OpCode::LoadMethod
                                && op.method_site() == Some(site) =>
                        {
                            Some(name.clone())
                        }
                        _ => None,
                    })?;
                let mut image = vec![0u8; method_site_record_size(name.len())];
                write_method_site_record_image(name.as_bytes(), &mut image);
                let context = self.backend.context;
                let ty = context.i8_type().array_type(image.len() as u32);
                let global = self.backend.module.add_global(ty, None, &symbol);
                global.set_linkage(inkwell::module::Linkage::Internal);
                global.set_initializer(&context.const_string(&image, false));
                global.set_constant(false);
                global.set_alignment(8);
                global
            }
        };
        Some(
            self.backend
                .builder
                .build_ptr_to_int(
                    global.as_pointer_value(),
                    self.backend.context.i64_type(),
                    "method_site_record",
                )
                .unwrap(),
        )
    }

    /// `LoadMethod` at method site `site`: the shared load body, whose miss
    /// is `molt_method_site_load(record, receiver, self_out)`.
    pub(super) fn emit_method_site_load_call(
        &mut self,
        site: u32,
        receiver: IntValue<'ctx>,
        self_out: IntValue<'ctx>,
    ) -> Option<IntValue<'ctx>> {
        let record = self.method_site_record(site)?;
        let admission = self
            .ensure_refcount_site(RefcountOperation::Retain)
            .admission;
        let helper = self.ensure_method_site_load_helper();
        Some(
            self.backend
                .builder
                .build_execution_call(
                    helper,
                    &[
                        record.into(),
                        receiver.into(),
                        self_out.into(),
                        admission.into(),
                    ],
                    "method_site_load",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value(),
        )
    }

    /// Whether a site call with `nargs` arguments after `self` has a cold
    /// fixed-arity runtime entry, and so a site body.
    pub(super) fn method_site_call_admits(nargs: usize) -> bool {
        nargs < COLD_CALLS.len()
    }

    /// `CallLoadedMethod` at method site `site` with `words` = [callable,
    /// self_or_null, args...], all adopted: the shared call body, whose miss
    /// is the runtime's loaded-method call. None when the call has no cold
    /// fixed-arity entry.
    pub(super) fn emit_method_site_call(
        &mut self,
        site: u32,
        words: &[IntValue<'ctx>],
    ) -> Option<IntValue<'ctx>> {
        let nargs = words.len().checked_sub(2)?;
        let cold = *COLD_CALLS.get(nargs)?;
        let record = self.method_site_record(site)?;
        let thread = self.thread_record_word();
        let admission = self
            .ensure_refcount_site(RefcountOperation::Retain)
            .admission;
        let helper = self.ensure_method_site_call_helper(nargs, cold);
        let mut args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            vec![thread.into(), record.into()];
        args.extend(
            words
                .iter()
                .map(|&word| inkwell::values::BasicMetadataValueEnum::from(word)),
        );
        args.push(admission.into());
        Some(
            self.backend
                .builder
                .build_execution_call(helper, &args, "method_site_call")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value(),
        )
    }

    /// An always-inlined helper `name` taking `params` words and returning
    /// one, with its body written by `body` on a fresh frame builder.
    fn method_site_helper(
        &self,
        name: &str,
        params: usize,
        recursion: bool,
        body: impl FnOnce(&mut LlvmFrameBuilder<'_, 'ctx>, &[IntValue<'ctx>]) -> IntValue<'ctx>,
    ) -> FunctionValue<'ctx> {
        let context = self.backend.context;
        let i64_type = context.i64_type();
        let logical_signature = i64_type.fn_type(&vec![i64_type.into(); params], false);
        let signature = prepend_execution_context(logical_signature);
        if let Some(function) = self.backend.module.get_compiled_function(name) {
            return require_llvm_function_type(name, function, signature);
        }
        let function = self.backend.module.add_compiled_function(
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
            enter: self.ensure_runtime_import(RuntimeFn::FrameRecordEnter),
            exit: self.ensure_runtime_import(RuntimeFn::FrameRecordExit),
            slow_blocks: Vec::new(),
            recursion: recursion.then(|| {
                (
                    self.ensure_runtime_import(RuntimeFn::RecursionEnterFast),
                    self.ensure_runtime_import(RuntimeFn::RecursionExitFast),
                )
            }),
        };
        let result = body(&mut sites, &words[..params - 1]);
        builder.build_return(Some(&result)).unwrap();
        // The locals were allocated in `entry` as the body asked for them.
        builder.position_at_end(entry);
        builder.build_unconditional_branch(body_block).unwrap();
        function
    }

    fn ensure_method_site_load_helper(&self) -> FunctionValue<'ctx> {
        let cold = self.ensure_runtime_import(RuntimeFn::MethodSiteLoad);
        self.method_site_helper("__molt_method_site_load", 4, false, |b, words| {
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

    fn ensure_method_site_call_helper(&self, nargs: usize, cold: RuntimeFn) -> FunctionValue<'ctx> {
        let cold = self.ensure_runtime_import(cold);
        // thread, record, callable, self, args..., admission
        self.method_site_helper(
            &format!("__molt_method_site_call_{nargs}"),
            4 + nargs + 1,
            true,
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
