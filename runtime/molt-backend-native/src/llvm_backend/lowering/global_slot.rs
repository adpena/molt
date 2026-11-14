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
use molt_codegen_abi::global_slot::{
    GLOBAL_SLOT_BASE_SHIFT, SlotBuilder, generate_global_slot_read,
};
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::static_cell::{
    CellBuilder, generate_static_cell_load, generate_static_cell_store,
};

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

impl<'ctx> FunctionLowering<'ctx, '_> {
    /// `StaticCellBase`: the module's cell block, a link-time constant
    /// address, as the inline int `GlobalSlotGet` takes for its base.
    pub(super) fn emit_static_cell_base(&mut self, op: &TirOp) {
        let Some(AttrValue::Str(module)) = op.attrs.get("s_value") else {
            self.record_fatal("StaticCellBase names no module".to_string());
            return;
        };
        let block = self.backend.static_cell_block(module);
        let i64_type = self.backend.context.i64_type();
        let builder = &self.backend.builder;
        let address = builder
            .build_ptr_to_int(block.as_pointer_value(), i64_type, "static_cells")
            .unwrap();
        let shifted = builder
            .build_right_shift(
                address,
                i64_type.const_int(u64::from(GLOBAL_SLOT_BASE_SHIFT), false),
                false,
                "static_cells_shifted",
            )
            .unwrap();
        let payload = builder
            .build_and(
                shifted,
                i64_type.const_int(molt_codegen_abi::INT_MASK, false),
                "static_cells_payload",
            )
            .unwrap();
        let base = builder
            .build_or(
                payload,
                i64_type.const_int(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_INT, false),
                "static_cells_base",
            )
            .unwrap();
        // The base is a boxed inline int, as `GlobalSlotBase` returns: a call
        // that takes it (the init's `molt_static_cells_bind`) passes it as is.
        let result = op.results[0];
        self.values.set_value(result, base.into());
        self.values.set_repr(
            result,
            crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
        );
    }

    /// `StaticCellLoad`: an init-dominated read (HF-846), one load from the
    /// module's block at the cell's offset. The cell keeps its reference.
    pub(super) fn emit_static_cell_load(&mut self, op: &TirOp) {
        let Some(slot) = op.global_slot() else {
            self.record_fatal("StaticCellLoad carries no slot".to_string());
            return;
        };
        let block = self.backend.static_cell_block(&slot.module);
        let context = self.backend.context;
        let builder = &self.backend.builder;
        let value = generate_static_cell_load(slot.index, |offset| {
            // SAFETY: the block holds one word per layout name and the
            // registry bounds the cell index (`validate_module_slots`).
            let pointer = unsafe {
                builder
                    .build_in_bounds_gep(
                        context.i8_type(),
                        block.as_pointer_value(),
                        &[context.i64_type().const_int(offset as u64, false)],
                        "static_cell",
                    )
                    .unwrap()
            };
            builder
                .build_load(context.i64_type(), pointer, "static_cell_word")
                .unwrap()
                .into_int_value()
        });
        let result = op.results[0];
        self.values.set_value(result, value.into());
        self.values.set_repr(
            result,
            crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
        );
    }

    /// `StaticCellSet` [base, value]: the shared store body for its cell.
    pub(super) fn emit_static_cell_set(&mut self, op: &TirOp) {
        let Some(slot) = op.global_slot() else {
            self.record_fatal("StaticCellSet carries no slot".to_string());
            return;
        };
        let helper = self.ensure_static_cell_store_helper(slot.index);
        let admission = self
            .ensure_refcount_site(RefcountOperation::Retain)
            .admission;
        let base = self.ensure_i64(self.resolve(op.operands[0]));
        // The cell holds a boxed word: a raw integer carrier is boxed first,
        // under the borrowed-operand custody every runtime argument takes.
        let mut custody = self.begin_borrowed_operands(&[op.operands[1]], "static_cell_set");
        let value = self.borrowed_operand(&mut custody, op.operands[1]);
        self.backend
            .builder
            .build_execution_call(helper, &[base.into(), value.into(), admission.into()], "")
            .unwrap();
        let none = self.none_word();
        self.finish_borrowed_operands(custody, none, "static_cell_set_result", |_| {});
    }

    fn ensure_static_cell_store_helper(&self, index: u32) -> FunctionValue<'ctx> {
        let name = format!("__molt_static_cell_store_{index}");
        let context = self.backend.context;
        let i64_type = context.i64_type();
        let logical_signature = context
            .void_type()
            .fn_type(&[i64_type.into(), i64_type.into(), i64_type.into()], false);
        let signature = prepend_execution_context(logical_signature);
        if let Some(function) = self.backend.module.get_compiled_function(&name) {
            return require_llvm_function_type(&name, function, signature);
        }
        let retain = self.ensure_refcount_helper(RefcountOperation::Retain);
        let release = self.ensure_refcount_helper(RefcountOperation::Release);
        let function = self.backend.module.add_compiled_function(
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

    /// `GlobalSlotBase`: the runtime entry, once per activation after GVN/LICM.
    pub(super) fn emit_global_slot_base(&mut self, op: &TirOp) {
        let callee = self.ensure_runtime_import(RuntimeFn::GlobalSlotBase);
        self.emit_borrowed_runtime_call(
            op,
            callee,
            &[RuntimeArg::Operand(op.operands[0])],
            RuntimeResultCustody::Unowned,
            "global_slot_base",
            RuntimeFn::GlobalSlotBase.symbol(),
        );
    }

    /// `GlobalSlotGet` [base, module, name]: the slot read body for its index.
    pub(super) fn emit_global_slot_get(&mut self, op: &TirOp) {
        let Some(slot) = op.global_slot() else {
            self.record_fatal("GlobalSlotGet carries no slot".to_string());
            return;
        };
        let helper = self.ensure_global_slot_read_helper(slot.index);
        let admission = self
            .ensure_refcount_site(RefcountOperation::Retain)
            .admission;
        let base = self.ensure_i64(self.resolve(op.operands[0]));
        let module = self.ensure_i64(self.resolve(op.operands[1]));
        let name = self.ensure_i64(self.resolve(op.operands[2]));
        let value = self
            .backend
            .builder
            .build_execution_call(
                helper,
                &[base.into(), module.into(), name.into(), admission.into()],
                "global_slot",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        self.bind_owned_runtime_result(op, value.into());
    }

    fn ensure_global_slot_read_helper(&self, index: u32) -> FunctionValue<'ctx> {
        let name = format!("__molt_global_slot_read_{index}");
        let i64_type = self.backend.context.i64_type();
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
        if let Some(function) = self.backend.module.get_compiled_function(&name) {
            return require_llvm_function_type(&name, function, signature);
        }
        let retain = self.ensure_refcount_helper(RefcountOperation::Retain);
        let cold = self.ensure_runtime_import(RuntimeFn::ModuleGetGlobal);
        let function = self.backend.module.add_compiled_function(
            &name,
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
        let builder: Builder<'ctx> = self.backend.context.create_builder();
        builder.position_at_end(self.backend.context.append_basic_block(function, "entry"));
        // Parameter 0 is the execution context.
        let param = |n: u32| function.get_nth_param(n).unwrap().into_int_value();
        let (base, module, global, admission) = (param(1), param(2), param(3), param(4));
        let mut emitter = LlvmSlotBuilder {
            words: LlvmWords {
                context: self.backend.context,
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
