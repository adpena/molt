//! LLVM instruction selection for the shared module global slot read
//! (call_anatomy.md 4.5; `molt_codegen_abi::global_slot` owns the meaning).
//! As the refcount transitions do, each slot index gets one always-inlined
//! internal body, so the read's explicit blocks never split the caller's
//! TIR block and its phi edges stay intact.
use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use inkwell::builder::Builder;
use inkwell::values::{IntValue, PointerValue};
use molt_codegen_abi::global_slot::{SlotBuilder, generate_global_slot_read};
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::static_cell::{CellBuilder, generate_static_cell_store};

struct LlvmSlotBuilder<'a, 'ctx> {
    words: LlvmWords<'a, 'ctx>,
    retain: FunctionValue<'ctx>,
    /// The release helper, present in a static cell store body.
    release: Option<FunctionValue<'ctx>>,
    admission: IntValue<'ctx>,
}

impl<'ctx> LlvmSlotBuilder<'_, 'ctx> {
    fn word_pointer(&self, address: IntValue<'ctx>, offset: i32) -> PointerValue<'ctx> {
        let context = self.words.context;
        let builder = self.words.builder;
        let at = builder
            .build_int_add(
                address,
                context.i64_type().const_int(offset as u64, true),
                "slot_address",
            )
            .unwrap();
        builder
            .build_int_to_ptr(
                at,
                context.ptr_type(inkwell::AddressSpace::default()),
                "slot_pointer",
            )
            .unwrap()
    }
}

impl<'a, 'ctx> molt_codegen_abi::word::WordBuilder for LlvmSlotBuilder<'a, 'ctx> {
    molt_codegen_abi::forward_word_builder!(words: LlvmWords<'a, 'ctx>);
}

impl<'ctx> super::LlvmBranchWords<'ctx> for LlvmSlotBuilder<'_, 'ctx> {
    fn llvm(&self) -> LlvmWords<'_, 'ctx> {
        self.words
    }
}

impl<'ctx> molt_codegen_abi::word::BranchBuilder for LlvmSlotBuilder<'_, 'ctx> {
    fn choose(
        &mut self,
        condition: IntValue<'ctx>,
        yes: impl FnOnce(&mut Self) -> IntValue<'ctx>,
        no: impl FnOnce(&mut Self) -> IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        super::llvm_choose(self, condition, yes, no)
    }
}

impl<'ctx> SlotBuilder for LlvmSlotBuilder<'_, 'ctx> {
    fn load_word(&mut self, address: IntValue<'ctx>, offset: i32) -> IntValue<'ctx> {
        let pointer = self.word_pointer(address, offset);
        self.words
            .builder
            .build_load(self.words.context.i64_type(), pointer, "slot_word")
            .unwrap()
            .into_int_value()
    }

    fn retain(&mut self, word: IntValue<'ctx>) {
        self.words
            .builder
            .build_execution_call(self.retain, &[word.into(), self.admission.into()], "")
            .unwrap();
    }
}

impl<'ctx> CellBuilder for LlvmSlotBuilder<'_, 'ctx> {
    fn swap_word(
        &mut self,
        address: IntValue<'ctx>,
        offset: i32,
        value: IntValue<'ctx>,
        atomic: bool,
    ) -> IntValue<'ctx> {
        let pointer = self.word_pointer(address, offset);
        let builder = self.words.builder;
        if atomic {
            return builder
                .build_atomicrmw(
                    inkwell::AtomicRMWBinOp::Xchg,
                    pointer,
                    value,
                    inkwell::AtomicOrdering::AcquireRelease,
                )
                .unwrap();
        }
        let old = builder
            .build_load(self.words.context.i64_type(), pointer, "cell_old")
            .unwrap()
            .into_int_value();
        builder.build_store(pointer, value).unwrap();
        old
    }

    fn release(&mut self, word: IntValue<'ctx>) {
        let release = self.release.expect("a static cell store body releases");
        self.words
            .builder
            .build_execution_call(release, &[word.into(), self.admission.into()], "")
            .unwrap();
    }
}

impl<'ctx> LlvmBackend<'ctx> {
    /// The static cell store body for cell `index`: `(context, base, value,
    /// admission) -> void`.
    pub(super) fn static_cell_store_helper(&self, index: u32) -> FunctionValue<'ctx> {
        let name = format!("__molt_static_cell_store_{index}");
        let context = self.context;
        let i64_type = context.i64_type();
        let logical_signature = context
            .void_type()
            .fn_type(&[i64_type.into(), i64_type.into(), i64_type.into()], false);
        let signature = prepend_execution_context(logical_signature);
        if let Some(function) = self.module.get_compiled_function(&name) {
            return require_llvm_function_type(&name, function, signature);
        }
        let retain = self.ensure_refcount_helper(RefcountOperation::Retain);
        let release = self.ensure_refcount_helper(RefcountOperation::Release);
        let function = self.module.add_compiled_function(
            &name,
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
        let builder: Builder<'ctx> = context.create_builder();
        builder.position_at_end(context.append_basic_block(function, "entry"));
        // Parameter 0 is the execution context.
        let param = |n: u32| function.get_nth_param(n).unwrap().into_int_value();
        let (base, value, admission) = (param(1), param(2), param(3));
        let mut emitter = LlvmSlotBuilder {
            words: LlvmWords {
                context,
                builder: &builder,
            },
            retain,
            release: Some(release),
            admission,
        };
        generate_static_cell_store(
            &mut emitter,
            base,
            index,
            value,
            nanbox::MOLT_REFCOUNT_ATOMIC,
        );
        builder.build_return(None).unwrap();
        function
    }

    /// The global slot read body for slot `index`: `(context, base, module,
    /// name, admission) -> owned value`; its miss is `molt_module_get_global`.
    pub(super) fn global_slot_read_helper(&self, index: u32) -> FunctionValue<'ctx> {
        let name = format!("__molt_global_slot_read_{index}");
        let i64_type = self.context.i64_type();
        let logical_signature = i64_type.fn_type(
            &[
                i64_type.into(),
                i64_type.into(),
                i64_type.into(),
                i64_type.into(),
            ],
            false,
        );
        let signature = prepend_execution_context(logical_signature);
        if let Some(function) = self.module.get_compiled_function(&name) {
            return require_llvm_function_type(&name, function, signature);
        }
        let retain = self.ensure_refcount_helper(RefcountOperation::Retain);
        let cold = self.runtime_import(RuntimeFn::ModuleGetGlobal);
        let function = self.module.add_compiled_function(
            &name,
            logical_signature,
            Some(inkwell::module::Linkage::Internal),
        );
        function.add_attribute(
            AttributeLoc::Function,
            self.context.create_enum_attribute(
                inkwell::attributes::Attribute::get_named_enum_kind_id("alwaysinline"),
                0,
            ),
        );
        let builder: Builder<'ctx> = self.context.create_builder();
        builder.position_at_end(self.context.append_basic_block(function, "entry"));
        // Parameter 0 is the execution context.
        let param = |n: u32| function.get_nth_param(n).unwrap().into_int_value();
        let (base, module, global, admission) = (param(1), param(2), param(3), param(4));
        let mut emitter = LlvmSlotBuilder {
            words: LlvmWords {
                context: self.context,
                builder: &builder,
            },
            retain,
            release: None,
            admission,
        };
        let value = generate_global_slot_read(&mut emitter, base, index, |b| {
            b.words
                .builder
                .build_execution_call(cold, &[module.into(), global.into()], "global_by_name")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value()
        });
        builder.build_return(Some(&value)).unwrap();
        function
    }
}

impl<'ctx> molt_codegen_abi::module_ready::ReadyBuilder for LlvmSlotBuilder<'_, 'ctx> {
    fn add(&mut self, left: IntValue<'ctx>, right: IntValue<'ctx>) -> IntValue<'ctx> {
        self.words
            .builder
            .build_int_add(left, right, "ready_address")
            .unwrap()
    }

    fn load_acquire(&mut self, address: IntValue<'ctx>, offset: i32) -> IntValue<'ctx> {
        let pointer = self.word_pointer(address, offset);
        let load = self
            .words
            .builder
            .build_load(self.words.context.i64_type(), pointer, "ready_word")
            .unwrap();
        let instruction = inkwell::values::BasicValue::as_instruction_value(&load).unwrap();
        instruction
            .set_atomic_ordering(inkwell::AtomicOrdering::Acquire)
            .unwrap();
        instruction.set_alignment(8).unwrap();
        load.into_int_value()
    }
}

impl<'ctx> LlvmBackend<'ctx> {
    /// The closed-import body (`molt_codegen_abi::module_ready`): `(context,
    /// id, admission) -> owned module`.
    pub(super) fn module_import_closed_helper(&self) -> FunctionValue<'ctx> {
        let name = "__molt_module_import_closed";
        let i64_type = self.context.i64_type();
        let logical_signature = i64_type.fn_type(&[i64_type.into(), i64_type.into()], false);
        let signature = prepend_execution_context(logical_signature);
        if let Some(function) = self.module.get_compiled_function(name) {
            return require_llvm_function_type(name, function, signature);
        }
        let retain = self.ensure_refcount_helper(RefcountOperation::Retain);
        let cold = self.runtime_import(RuntimeFn::ModuleEnsure);
        let block = self.module_ready_block();
        let function = self.module.add_compiled_function(
            name,
            logical_signature,
            Some(inkwell::module::Linkage::Internal),
        );
        function.add_attribute(
            AttributeLoc::Function,
            self.context.create_enum_attribute(
                inkwell::attributes::Attribute::get_named_enum_kind_id("alwaysinline"),
                0,
            ),
        );
        let builder: Builder<'ctx> = self.context.create_builder();
        builder.position_at_end(self.context.append_basic_block(function, "entry"));
        // Parameter 0 is the execution context.
        let param = |n: u32| function.get_nth_param(n).unwrap().into_int_value();
        let (id, admission) = (param(1), param(2));
        let address = builder
            .build_ptr_to_int(block.as_pointer_value(), i64_type, "ready_block")
            .unwrap();
        let mut emitter = LlvmSlotBuilder {
            words: LlvmWords {
                context: self.context,
                builder: &builder,
            },
            retain,
            release: None,
            admission,
        };
        let value = molt_codegen_abi::module_ready::generate_module_import_closed(
            &mut emitter,
            address,
            id,
            |b, id| {
                b.words
                    .builder
                    .build_execution_call(cold, &[id.into()], "module_ensure")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value()
            },
        );
        builder.build_return(Some(&value)).unwrap();
        function
    }
}
