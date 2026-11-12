//! Shared RC generation in always-inlined LLVM bodies. Keeping its explicit
//! blocks in a separate IR function leaves the caller's TIR/phi CFG intact.
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use inkwell::builder::Builder;
use inkwell::values::IntValue;
use molt_codegen_abi::inline_refcount::{
    RefcountBuilder, RefcountOperation, RefcountStorage, generate_refcount,
};
use molt_codegen_abi::word::WordBuilder;

#[derive(Clone, Copy)]
pub(super) struct LlvmRefcountSite<'ctx> {
    pub(super) function: FunctionValue<'ctx>,
    pub(super) admission: IntValue<'ctx>,
}

impl<'ctx> LlvmRefcountSite<'ctx> {
    pub(super) fn emit(self, builder: &Builder<'ctx>, value: BasicValueEnum<'ctx>) {
        builder
            .build_execution_call(self.function, &[value.into(), self.admission.into()], "")
            .unwrap();
    }
}

impl<'ctx> FunctionLowering<'ctx, '_> {
    pub(super) fn emit_refcount_admission(&self, builder: &Builder<'ctx>) -> IntValue<'ctx> {
        if nanbox::MOLT_REFCOUNT_ATOMIC {
            return self.backend.context.i64_type().const_zero();
        }
        builder
            .build_execution_call(
                self.ensure_runtime_import(
                    molt_ir::runtime_fn_generated::RuntimeFn::RefcountAdmission,
                ),
                &[],
                "rc_admission",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }

    pub(super) fn ensure_refcount_site(
        &self,
        operation: RefcountOperation,
    ) -> LlvmRefcountSite<'ctx> {
        let admission = if let Some(value) = self.refcount_admission.get() {
            value
        } else {
            let entry = self.entry_block_builder(&self.backend.builder);
            let value = self.emit_refcount_admission(&entry);
            self.refcount_admission.set(Some(value));
            value
        };
        LlvmRefcountSite {
            function: self.ensure_refcount_helper(operation),
            admission,
        }
    }

    pub(super) fn ensure_refcount_helper(
        &self,
        operation: RefcountOperation,
    ) -> FunctionValue<'ctx> {
        let name = match operation {
            RefcountOperation::Retain => "__molt_inline_retain",
            RefcountOperation::Release => "__molt_inline_release",
        };
        let i64_type = self.backend.context.i64_type();
        let logical_signature = self
            .backend
            .context
            .void_type()
            .fn_type(&[i64_type.into(), i64_type.into()], false);
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
            self.backend.context.create_enum_attribute(
                inkwell::attributes::Attribute::get_named_enum_kind_id("alwaysinline"),
                0,
            ),
        );
        let builder = self.backend.context.create_builder();
        builder.position_at_end(self.backend.context.append_basic_block(function, "entry"));
        let slow = self.ensure_runtime_import(match operation {
            RefcountOperation::Retain => RuntimeFn::IncRefObj,
            RefcountOperation::Release => RuntimeFn::DecRefObj,
        });
        let mut emitter = LlvmRefcountBuilder {
            words: LlvmWords {
                context: self.backend.context,
                builder: &builder,
            },
            backend: self.backend,
            function,
            slow_block: None,
        };
        generate_refcount(
            &mut emitter,
            function.get_nth_param(1).unwrap().into_int_value(),
            function.get_nth_param(2).unwrap().into_int_value(),
            operation,
            RefcountStorage::for_build(nanbox::MOLT_REFCOUNT_ATOMIC),
        );
        builder.build_return(None).unwrap();
        if let Some(block) = emitter.slow_block {
            builder.position_at_end(block);
            builder
                .build_execution_call(slow, &[function.get_nth_param(1).unwrap().into()], "")
                .unwrap();
            builder.build_return(None).unwrap();
        }
        function
    }
}

struct LlvmRefcountBuilder<'a, 'ctx> {
    words: LlvmWords<'a, 'ctx>,
    backend: &'a LlvmBackend<'ctx>,
    function: FunctionValue<'ctx>,
    slow_block: Option<BasicBlock<'ctx>>,
}

impl<'ctx> LlvmRefcountBuilder<'_, 'ctx> {
    fn address(&self, pointer: IntValue<'ctx>, offset: i32) -> inkwell::values::PointerValue<'ctx> {
        let address = self
            .words
            .builder
            .build_int_add(
                pointer,
                self.backend
                    .context
                    .i64_type()
                    .const_int(offset as u64, true),
                "rc_address",
            )
            .unwrap();
        self.words
            .builder
            .build_int_to_ptr(
                address,
                self.backend
                    .context
                    .ptr_type(inkwell::AddressSpace::default()),
                "rc_header",
            )
            .unwrap()
    }
}

impl<'a, 'ctx> WordBuilder for LlvmRefcountBuilder<'a, 'ctx> {
    molt_codegen_abi::forward_word_builder!(words: LlvmWords<'a, 'ctx>);
}

impl<'ctx> RefcountBuilder for LlvmRefcountBuilder<'_, 'ctx> {
    fn pointer_width(&self) -> nanbox::PointerWidth {
        nanbox::PointerWidth::Bits64
    }
    fn load_header_u32(&mut self, pointer: Self::Word, offset: i32) -> Self::Word {
        let value = self
            .words
            .builder
            .build_load(
                self.backend.context.i32_type(),
                self.address(pointer, offset),
                "rc_word",
            )
            .unwrap();
        self.words
            .builder
            .build_int_z_extend(
                value.into_int_value(),
                self.backend.context.i64_type(),
                "rc_extend",
            )
            .unwrap()
    }
    fn store_header_u32(&mut self, pointer: Self::Word, offset: i32, value: Self::Word) {
        let value = self
            .words
            .builder
            .build_int_truncate(value, self.backend.context.i32_type(), "rc_count")
            .unwrap();
        self.words
            .builder
            .build_store(self.address(pointer, offset), value)
            .unwrap();
    }
    fn slow_call(&mut self, _operation: RefcountOperation, _bits: Self::Word) {
        let block = *self.slow_block.get_or_insert_with(|| {
            self.backend
                .context
                .append_basic_block(self.function, "rc_slow")
        });
        self.words
            .builder
            .build_unconditional_branch(block)
            .unwrap();
    }
    fn if_else(
        &mut self,
        condition: Self::Predicate,
        yes: impl FnOnce(&mut Self),
        no: impl FnOnce(&mut Self),
    ) {
        let yes_block = self
            .backend
            .context
            .append_basic_block(self.function, "rc_yes");
        let no_block = self
            .backend
            .context
            .append_basic_block(self.function, "rc_no");
        let continuation = self
            .backend
            .context
            .append_basic_block(self.function, "rc_continue");
        self.words
            .builder
            .build_conditional_branch(condition, yes_block, no_block)
            .unwrap();
        self.words.builder.position_at_end(yes_block);
        yes(self);
        if self
            .words
            .builder
            .get_insert_block()
            .unwrap()
            .get_terminator()
            .is_none()
        {
            self.words
                .builder
                .build_unconditional_branch(continuation)
                .unwrap();
        }
        self.words.builder.position_at_end(no_block);
        no(self);
        if self
            .words
            .builder
            .get_insert_block()
            .unwrap()
            .get_terminator()
            .is_none()
        {
            self.words
                .builder
                .build_unconditional_branch(continuation)
                .unwrap();
        }
        self.words.builder.position_at_end(continuation);
    }
}
