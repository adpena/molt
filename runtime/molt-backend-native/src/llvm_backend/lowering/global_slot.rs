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
use inkwell::values::IntValue;
use molt_codegen_abi::global_slot::{SlotBuilder, generate_global_slot_read};
use molt_codegen_abi::inline_refcount::RefcountOperation;

struct LlvmSlotBuilder<'a, 'ctx> {
    words: LlvmWords<'a, 'ctx>,
    function: FunctionValue<'ctx>,
    retain: FunctionValue<'ctx>,
    admission: IntValue<'ctx>,
}

impl<'a, 'ctx> molt_codegen_abi::word::WordBuilder for LlvmSlotBuilder<'a, 'ctx> {
    molt_codegen_abi::forward_word_builder!(words: LlvmWords<'a, 'ctx>);
}

impl<'ctx> SlotBuilder for LlvmSlotBuilder<'_, 'ctx> {
    fn load_word(&mut self, address: IntValue<'ctx>, offset: i32) -> IntValue<'ctx> {
        let context = self.words.context;
        let builder = self.words.builder;
        let at = builder
            .build_int_add(
                address,
                context.i64_type().const_int(offset as u64, true),
                "slot_address",
            )
            .unwrap();
        let pointer = builder
            .build_int_to_ptr(
                at,
                context.ptr_type(inkwell::AddressSpace::default()),
                "slot_pointer",
            )
            .unwrap();
        builder
            .build_load(context.i64_type(), pointer, "slot_word")
            .unwrap()
            .into_int_value()
    }

    fn choose(
        &mut self,
        condition: IntValue<'ctx>,
        yes: impl FnOnce(&mut Self) -> IntValue<'ctx>,
        no: impl FnOnce(&mut Self) -> IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        let context = self.words.context;
        let yes_block = context.append_basic_block(self.function, "slot_bound");
        let no_block = context.append_basic_block(self.function, "slot_unbound");
        let join = context.append_basic_block(self.function, "slot_join");
        self.words
            .builder
            .build_conditional_branch(condition, yes_block, no_block)
            .unwrap();
        self.words.builder.position_at_end(yes_block);
        let taken = yes(self);
        let taken_end = self.words.builder.get_insert_block().unwrap();
        self.words.builder.build_unconditional_branch(join).unwrap();
        self.words.builder.position_at_end(no_block);
        let fallback = no(self);
        let fallback_end = self.words.builder.get_insert_block().unwrap();
        self.words.builder.build_unconditional_branch(join).unwrap();
        self.words.builder.position_at_end(join);
        let phi = self
            .words
            .builder
            .build_phi(context.i64_type(), "slot_value")
            .unwrap();
        phi.add_incoming(&[(&taken, taken_end), (&fallback, fallback_end)]);
        phi.as_basic_value().into_int_value()
    }

    fn retain(&mut self, word: IntValue<'ctx>) {
        self.words
            .builder
            .build_execution_call(self.retain, &[word.into(), self.admission.into()], "")
            .unwrap();
    }
}

impl<'ctx> FunctionLowering<'ctx, '_> {
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
        if let Some(function) = self.backend.module.get_execution_function(&name) {
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
            function,
            retain,
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
