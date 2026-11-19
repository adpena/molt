//! LLVM's implementation of the walker's builder trait
//! (`codegen_ssa::CodegenBuilder`): instruction selection only.

use inkwell::basic_block::BasicBlock;
use inkwell::values::{
    BasicValue, BasicValueEnum, FunctionValue, IntValue, PhiValue, PointerValue,
};
use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_codegen_abi::word::{BranchBuilder, WordBuilder};
use molt_ir::deterministic_hash::HashMap;
use molt_ir::runtime_fn_generated::RuntimeFn;
use std::cell::{Cell, RefCell};

use super::refcount::{INLINE_REFCOUNT_SITES_PER_FUNCTION, LlvmRefcountSite};
use super::{LlvmBranchWords, LlvmWords, llvm_choose};
use crate::codegen_ssa::{
    CheckedBinary, CodegenBuilder, FloatBinary, FloatComparison, IntBinary, IntComparison,
    MachineKind, SiteBody,
};
use crate::llvm_backend::LlvmBackend;
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;

/// The phis that stand for each block's parameters, in parameter order.
pub(in crate::llvm_backend) type BlockParams<'ctx> =
    RefCell<HashMap<BasicBlock<'ctx>, Vec<PhiValue<'ctx>>>>;

/// Per-function instruction-selection state: what one LLVM function reads
/// once and reuses, and how many reference-count sites it holds.
pub(in crate::llvm_backend) struct LlvmFunctionState<'ctx> {
    /// The invocation's thread record, loaded in the true entry.
    thread_record: Cell<Option<PointerValue<'ctx>>>,
    /// The refcount admission word, read once in the entry.
    refcount_admission: Cell<Option<IntValue<'ctx>>>,
    /// Refcount sites emitted in this function
    /// (`INLINE_REFCOUNT_SITES_PER_FUNCTION`).
    refcount_sites: Cell<usize>,
    /// Whether the function inlines fast paths (`CodeTemperature`).
    inline_fast_paths: bool,
    /// Private byte constants named so far, which numbers the next one.
    byte_constants: Cell<usize>,
    /// The pending-work word's address the record lends, loaded once in the
    /// entry right after the record; only functions that poll read it.
    pending_work_ptr: Cell<Option<PointerValue<'ctx>>>,
}

impl LlvmFunctionState<'_> {
    pub(in crate::llvm_backend) fn new(inline_fast_paths: bool) -> Self {
        Self {
            thread_record: Cell::new(None),
            refcount_admission: Cell::new(None),
            refcount_sites: Cell::new(0),
            inline_fast_paths,
            byte_constants: Cell::new(0),
            pending_work_ptr: Cell::new(None),
        }
    }
}

/// The LLVM builder at its current insertion point, with the backend that
/// owns the module and its declarations, the function's block parameters and
/// its per-function state.
#[derive(Clone, Copy)]
pub(in crate::llvm_backend) struct LlvmCodegen<'a, 'ctx> {
    pub(in crate::llvm_backend) words: LlvmWords<'a, 'ctx>,
    pub(in crate::llvm_backend) backend: &'a LlvmBackend<'ctx>,
    pub(in crate::llvm_backend) params: &'a BlockParams<'ctx>,
    pub(in crate::llvm_backend) state: &'a LlvmFunctionState<'ctx>,
}

/// A fresh builder in the target function's true entry, even before body
/// lowering positions the module's instruction builder.
pub(super) fn entry_builder<'ctx>(
    context: &'ctx inkwell::context::Context,
    function: FunctionValue<'ctx>,
) -> inkwell::builder::Builder<'ctx> {
    let builder = context.create_builder();
    let entry = function
        .get_first_basic_block()
        .expect("llvm function missing entry block");
    match entry.get_first_instruction() {
        Some(first) => builder.position_before(&first),
        None => builder.position_at_end(entry),
    }
    builder
}

impl<'ctx> LlvmCodegen<'_, 'ctx> {
    /// The function the builder is positioned in.
    fn function(&self) -> FunctionValue<'ctx> {
        self.words
            .builder
            .get_insert_block()
            .and_then(|block| block.get_parent())
            .expect("the builder is positioned in a function")
    }

    /// A builder positioned before the first instruction of the current
    /// function's entry block.
    pub(in crate::llvm_backend) fn entry_builder(&self) -> inkwell::builder::Builder<'ctx> {
        entry_builder(self.words.context, self.function())
    }

    pub(super) fn thread_record(&self) -> PointerValue<'ctx> {
        if let Some(record) = self.state.thread_record.get() {
            return record;
        }
        let builder = self.entry_builder();
        let context = self.words.context;
        let token = self.function().get_first_param().unwrap().into_int_value();
        let bits = super::refcount::thread_record_bits(&builder, context, token);
        let record = builder
            .build_int_to_ptr(
                bits,
                context.ptr_type(inkwell::AddressSpace::default()),
                "thread_record_ptr",
            )
            .unwrap();
        self.state.thread_record.set(Some(record));
        record
    }
    /// The pending-work word's address the thread record lends, loaded once
    /// in the entry right after the record (the entry builder inserts before
    /// the block's first instruction, which may precede the record).
    fn pending_work_ptr(&self) -> PointerValue<'ctx> {
        if let Some(word) = self.state.pending_work_ptr.get() {
            return word;
        }
        let record = self.thread_record();
        let context = self.words.context;
        let ptr_ty = context.ptr_type(inkwell::AddressSpace::default());
        let builder = context.create_builder();
        let defined = record
            .as_instruction()
            .expect("the thread record is an entry-block instruction");
        match defined.get_next_instruction() {
            Some(next) => builder.position_before(&next),
            None => builder
                .position_at_end(defined.get_parent().expect("the thread record has a block")),
        }
        // SAFETY: the field lies inside the thread record.
        let field = unsafe {
            builder
                .build_gep(
                    context.i8_type(),
                    record,
                    &[context.i64_type().const_int(
                        molt_codegen_abi::THREAD_RECORD_PENDING_WORK_PTR_OFFSET as u64,
                        false,
                    )],
                    "pending_work_field",
                )
                .unwrap()
        };
        let word = builder
            .build_load(context.i64_type(), field, "pending_work_address")
            .unwrap()
            .into_int_value();
        let word = builder
            .build_int_to_ptr(word, ptr_ty, "pending_work_ptr")
            .unwrap();
        self.state.pending_work_ptr.set(Some(word));
        word
    }

    /// One refcount site of this function: the admission word, read once in
    /// the entry, and the inline transition, or its one out-of-line copy once
    /// the function holds `INLINE_REFCOUNT_SITES_PER_FUNCTION` sites.
    pub(in crate::llvm_backend) fn llvm_refcount_site(
        &self,
        operation: RefcountOperation,
    ) -> LlvmRefcountSite<'ctx> {
        let admission = match self.state.refcount_admission.get() {
            Some(admission) => admission,
            None => {
                let admission = self.backend.emit_refcount_admission(&self.entry_builder());
                self.state.refcount_admission.set(Some(admission));
                admission
            }
        };
        let sites = self.state.refcount_sites.get();
        self.state.refcount_sites.set(sites + 1);
        let function = if self.state.inline_fast_paths && sites < INLINE_REFCOUNT_SITES_PER_FUNCTION
        {
            self.backend.ensure_refcount_helper(operation)
        } else {
            self.backend.ensure_outlined_refcount_helper(operation)
        };
        LlvmRefcountSite {
            function,
            admission,
        }
    }
    /// Add this block's edge to each parameter phi of `target`.
    fn pass_arguments(&self, target: BasicBlock<'ctx>, args: &[BasicValueEnum<'ctx>]) {
        let source = self
            .words
            .builder
            .get_insert_block()
            .expect("a branch is built inside a block");
        // A block with no parameter phis is unreachable in the walk's order:
        // nothing lowers it, so no phi takes the edge.
        if let Some(phis) = self.params.borrow().get(&target) {
            assert_eq!(
                phis.len(),
                args.len(),
                "an edge passes one argument per block parameter"
            );
            for (phi, arg) in phis.iter().zip(args) {
                phi.add_incoming(&[(arg, source)]);
            }
        }
    }

    fn machine_type(&self, kind: MachineKind) -> inkwell::types::BasicTypeEnum<'ctx> {
        let context = self.words.context;
        match kind {
            MachineKind::Bit => context.bool_type().into(),
            MachineKind::NarrowInt => context.i32_type().into(),
            MachineKind::Word => context.i64_type().into(),
            MachineKind::Float => context.f64_type().into(),
            MachineKind::Address => context.ptr_type(inkwell::AddressSpace::default()).into(),
        }
    }
}

impl<'a, 'ctx> WordBuilder for LlvmCodegen<'a, 'ctx> {
    molt_codegen_abi::forward_word_builder!(words: LlvmWords<'a, 'ctx>);
}

impl<'ctx> LlvmBranchWords<'ctx> for LlvmCodegen<'_, 'ctx> {
    fn llvm(&self) -> LlvmWords<'_, 'ctx> {
        self.words
    }
}

impl<'ctx> BranchBuilder for LlvmCodegen<'_, 'ctx> {
    fn choose(
        &mut self,
        condition: Self::Predicate,
        yes: impl FnOnce(&mut Self) -> Self::Word,
        no: impl FnOnce(&mut Self) -> Self::Word,
    ) -> Self::Word {
        llvm_choose(self, condition, yes, no)
    }
}

impl<'ctx> CodegenBuilder for LlvmCodegen<'_, 'ctx> {
    type Value = BasicValueEnum<'ctx>;
    type Block = BasicBlock<'ctx>;
    type Slot = PointerValue<'ctx>;
    type Callee = FunctionValue<'ctx>;
    type RefcountSite = LlvmRefcountSite<'ctx>;

    fn kind(&self, value: Self::Value) -> MachineKind {
        match value {
            BasicValueEnum::IntValue(int) => match int.get_type().get_bit_width() {
                1 => MachineKind::Bit,
                64 => MachineKind::Word,
                width if width < 64 => MachineKind::NarrowInt,
                width => panic!("no lowered value is a {width}-bit integer"),
            },
            BasicValueEnum::FloatValue(_) => MachineKind::Float,
            BasicValueEnum::PointerValue(_) => MachineKind::Address,
            other => panic!("{other:?} is not a scalar machine value"),
        }
    }
    fn word_value(&self, word: IntValue<'ctx>) -> Self::Value {
        word.into()
    }
    fn bit_value(&self, bit: IntValue<'ctx>) -> Self::Value {
        bit.into()
    }
    fn expect_word(&self, value: Self::Value) -> IntValue<'ctx> {
        value.into_int_value()
    }
    fn expect_bit(&self, value: Self::Value) -> IntValue<'ctx> {
        value.into_int_value()
    }

    fn zero_extend(&mut self, value: Self::Value) -> IntValue<'ctx> {
        self.words
            .builder
            .build_int_z_extend(
                value.into_int_value(),
                self.words.context.i64_type(),
                "zext_i64",
            )
            .unwrap()
    }
    fn low_bit(&mut self, word: IntValue<'ctx>) -> IntValue<'ctx> {
        self.words
            .builder
            .build_int_truncate(word, self.words.context.bool_type(), "low_bit")
            .unwrap()
    }
    fn float_to_bits(&mut self, value: Self::Value) -> IntValue<'ctx> {
        self.words
            .builder
            .build_bit_cast(
                value.into_float_value(),
                self.words.context.i64_type(),
                "f2i",
            )
            .unwrap()
            .into_int_value()
    }
    fn bits_to_float(&mut self, word: IntValue<'ctx>) -> Self::Value {
        self.words
            .builder
            .build_bit_cast(word, self.words.context.f64_type(), "i2f")
            .unwrap()
    }
    fn address_to_word(&mut self, value: Self::Value) -> IntValue<'ctx> {
        self.words
            .builder
            .build_ptr_to_int(
                value.into_pointer_value(),
                self.words.context.i64_type(),
                "ptr2i",
            )
            .unwrap()
    }
    fn word_to_address(&mut self, word: IntValue<'ctx>) -> Self::Value {
        self.words
            .builder
            .build_int_to_ptr(
                word,
                self.words
                    .context
                    .ptr_type(inkwell::AddressSpace::default()),
                "i2ptr",
            )
            .unwrap()
            .into()
    }

    fn int_binary(
        &mut self,
        op: IntBinary,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
        no_signed_wrap: bool,
    ) -> IntValue<'ctx> {
        let builder = self.words.builder;
        match (op, no_signed_wrap) {
            (IntBinary::Add, true) => builder.build_int_nsw_add(left, right, "add"),
            (IntBinary::Add, false) => builder.build_int_add(left, right, "add"),
            (IntBinary::Sub, true) => builder.build_int_nsw_sub(left, right, "sub"),
            (IntBinary::Sub, false) => builder.build_int_sub(left, right, "sub"),
            (IntBinary::Mul, true) => builder.build_int_nsw_mul(left, right, "mul"),
            (IntBinary::Mul, false) => builder.build_int_mul(left, right, "mul"),
            (IntBinary::And, _) => builder.build_and(left, right, "band"),
            (IntBinary::Or, _) => builder.build_or(left, right, "bor"),
            (IntBinary::Xor, _) => builder.build_xor(left, right, "bxor"),
            (IntBinary::ShiftLeft, _) => builder.build_left_shift(left, right, "shl"),
            (IntBinary::ShiftRightLogical, _) => {
                builder.build_right_shift(left, right, false, "lshr")
            }
            (IntBinary::ShiftRightArithmetic, _) => {
                builder.build_right_shift(left, right, true, "shr")
            }
            (IntBinary::SignedQuotient, _) => builder.build_int_signed_div(left, right, "sdiv"),
            (IntBinary::SignedRemainder, _) => builder.build_int_signed_rem(left, right, "srem"),
        }
        .unwrap()
    }
    fn int_checked(
        &mut self,
        op: CheckedBinary,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let name = match op {
            CheckedBinary::Add => "llvm.sadd.with.overflow",
            CheckedBinary::Mul => "llvm.smul.with.overflow",
        };
        let i64_ty = self.words.context.i64_type();
        let declaration = inkwell::intrinsics::Intrinsic::find(name)
            .unwrap_or_else(|| panic!("{name} is an LLVM intrinsic"))
            .get_declaration(&self.backend.module, &[i64_ty.into()])
            .unwrap_or_else(|| panic!("{name}.i64 declares"));
        let pair = self
            .words
            .builder
            .build_execution_call(declaration, &[left.into(), right.into()], "checked")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_struct_value();
        let builder = self.words.builder;
        let value = builder
            .build_extract_value(pair, 0, "checked_value")
            .unwrap();
        let overflow = builder.build_extract_value(pair, 1, "overflow").unwrap();
        (value.into_int_value(), overflow.into_int_value())
    }
    fn is_zero(&mut self, value: Self::Value) -> IntValue<'ctx> {
        let value = value.into_int_value();
        self.words
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                value,
                value.get_type().const_zero(),
                "is_zero",
            )
            .unwrap()
    }
    fn int_to_float(&mut self, word: IntValue<'ctx>) -> Self::Value {
        self.words
            .builder
            .build_signed_int_to_float(word, self.words.context.f64_type(), "sitofp")
            .unwrap()
            .into()
    }
    fn select(
        &mut self,
        condition: IntValue<'ctx>,
        yes: Self::Value,
        no: Self::Value,
    ) -> Self::Value {
        self.words
            .builder
            .build_select(condition, yes, no, "select")
            .unwrap()
    }
    fn bit_xor(&mut self, left: IntValue<'ctx>, right: IntValue<'ctx>) -> IntValue<'ctx> {
        self.words
            .builder
            .build_xor(left, right, "bit_xor")
            .unwrap()
    }
    fn int_compare(
        &mut self,
        comparison: IntComparison,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        let predicate = match comparison {
            IntComparison::Equal => inkwell::IntPredicate::EQ,
            IntComparison::NotEqual => inkwell::IntPredicate::NE,
            IntComparison::SignedLess => inkwell::IntPredicate::SLT,
            IntComparison::SignedLessOrEqual => inkwell::IntPredicate::SLE,
            IntComparison::SignedGreater => inkwell::IntPredicate::SGT,
            IntComparison::SignedGreaterOrEqual => inkwell::IntPredicate::SGE,
        };
        self.words
            .builder
            .build_int_compare(predicate, left, right, "int_test")
            .unwrap()
    }
    fn int_negate(&mut self, word: IntValue<'ctx>) -> IntValue<'ctx> {
        self.words.builder.build_int_neg(word, "neg").unwrap()
    }
    fn int_invert(&mut self, word: IntValue<'ctx>) -> IntValue<'ctx> {
        self.words.builder.build_not(word, "invert").unwrap()
    }
    fn bit_not(&mut self, bit: IntValue<'ctx>) -> IntValue<'ctx> {
        self.words.builder.build_not(bit, "not").unwrap()
    }

    fn float_binary(
        &mut self,
        op: FloatBinary,
        left: Self::Value,
        right: Self::Value,
        fast_math: bool,
    ) -> Self::Value {
        let builder = self.words.builder;
        let (left, right) = (left.into_float_value(), right.into_float_value());
        let value = match op {
            FloatBinary::Add => builder.build_float_add(left, right, "fadd"),
            FloatBinary::Sub => builder.build_float_sub(left, right, "fsub"),
            FloatBinary::Mul => builder.build_float_mul(left, right, "fmul"),
            FloatBinary::Divide => builder.build_float_div(left, right, "fdiv"),
            FloatBinary::Remainder => builder.build_float_rem(left, right, "frem"),
        }
        .unwrap();
        // A constant-folded result is not an instruction and takes no flags.
        if fast_math && let Some(instruction) = value.as_instruction() {
            instruction
                .set_fast_math_flags(super::llvm_fast_math_all())
                .unwrap();
        }
        value.into()
    }
    fn float_negate(&mut self, value: Self::Value) -> Self::Value {
        self.words
            .builder
            .build_float_neg(value.into_float_value(), "fneg")
            .unwrap()
            .into()
    }
    fn float_constant(&mut self, value: f64) -> Self::Value {
        self.words.context.f64_type().const_float(value).into()
    }
    fn float_compare(
        &mut self,
        comparison: FloatComparison,
        left: Self::Value,
        right: Self::Value,
    ) -> IntValue<'ctx> {
        let predicate = match comparison {
            FloatComparison::OrderedEqual => inkwell::FloatPredicate::OEQ,
            FloatComparison::UnorderedOrNotEqual => inkwell::FloatPredicate::UNE,
            FloatComparison::OrderedLess => inkwell::FloatPredicate::OLT,
            FloatComparison::OrderedLessOrEqual => inkwell::FloatPredicate::OLE,
            FloatComparison::OrderedGreater => inkwell::FloatPredicate::OGT,
            FloatComparison::OrderedGreaterOrEqual => inkwell::FloatPredicate::OGE,
        };
        self.words
            .builder
            .build_float_compare(
                predicate,
                left.into_float_value(),
                right.into_float_value(),
                "float_test",
            )
            .unwrap()
    }

    fn runtime_callee(&mut self, function: RuntimeFn) -> FunctionValue<'ctx> {
        self.backend
            .runtime_imports
            .get(self.words.context, &self.backend.module, function)
    }

    /// The module's declaration of the compiled function, from the backend's
    /// own copy of the row (`linked_compiled_function`).
    fn function_callee(
        &mut self,
        symbol: &str,
        abi: &molt_ir::linkage_abi::NativeFunctionLinkageAbi,
    ) -> FunctionValue<'ctx> {
        debug_assert_eq!(
            self.backend.function_linkage_abis.get(symbol),
            Some(abi),
            "`{symbol}` is declared by its program row"
        );
        crate::llvm_backend::lowering::linked_compiled_function(self.backend, symbol)
    }

    fn function_address(&mut self, callee: FunctionValue<'ctx>) -> IntValue<'ctx> {
        self.words
            .builder
            .build_ptr_to_int(
                callee.as_global_value().as_pointer_value(),
                self.words.context.i64_type(),
                "target_code",
            )
            .unwrap()
    }

    fn call(
        &mut self,
        callee: FunctionValue<'ctx>,
        args: &[BasicValueEnum<'ctx>],
    ) -> Option<BasicValueEnum<'ctx>> {
        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            args.iter().map(|&arg| arg.into()).collect();
        self.words
            .builder
            .build_execution_call(callee, &args, "call")
            .unwrap()
            .try_as_basic_value()
            .basic()
    }

    /// The load is volatile and atomic (monotonic), so no LLVM pass merges
    /// two reads or hoists one out of a loop.
    fn async_work_pending(&mut self) -> IntValue<'ctx> {
        let word_ptr = self.pending_work_ptr();
        let context = self.words.context;
        let builder = self.words.builder;
        let word = builder
            .build_load(context.i32_type(), word_ptr, "pending_work")
            .unwrap();
        let load = word.as_instruction_value().unwrap();
        load.set_alignment(4).unwrap();
        load.set_atomic_ordering(inkwell::AtomicOrdering::Monotonic)
            .unwrap();
        load.set_volatile(true).unwrap();
        builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                word.into_int_value(),
                context.i32_type().const_zero(),
                "has_async_work",
            )
            .unwrap()
    }

    fn recursion_charge(
        &mut self,
        charge: crate::codegen_ssa::RecursionCharge,
        thread: IntValue<'ctx>,
    ) -> Option<IntValue<'ctx>> {
        use crate::codegen_ssa::RecursionCharge;
        use molt_codegen_abi::thread_record::{
            generate_recursion_admits, generate_recursion_count, generate_recursion_release,
        };
        match charge {
            RecursionCharge::Admits => Some(generate_recursion_admits(&mut self.words, thread)),
            RecursionCharge::Count => {
                generate_recursion_count(&mut self.words, thread);
                None
            }
            RecursionCharge::Release => {
                generate_recursion_release(&mut self.words, thread);
                None
            }
        }
    }

    fn thread_record_word(&mut self) -> Self::Word {
        let record = self.thread_record();
        self.words
            .builder
            .build_ptr_to_int(record, self.words.context.i64_type(), "thread_record_word")
            .unwrap()
    }
    fn load_byte(&mut self, address: IntValue<'ctx>, offset: i32) -> Self::Value {
        let pointer = self.words.address(address, offset);
        self.words
            .builder
            .build_load(self.words.context.i8_type(), pointer, "record_byte")
            .unwrap()
    }
    fn call_cold(
        &mut self,
        callee: FunctionValue<'ctx>,
        args: &[Self::Value],
    ) -> Option<Self::Value> {
        let args: Vec<_> = args.iter().copied().map(Into::into).collect();
        let call = self
            .words
            .builder
            .build_execution_call(callee, &args, "cold_call")
            .unwrap();
        call.add_attribute(
            inkwell::attributes::AttributeLoc::Function,
            self.words.context.create_enum_attribute(
                inkwell::attributes::Attribute::get_named_enum_kind_id("cold"),
                0,
            ),
        );
        call.try_as_basic_value().basic()
    }

    fn create_block(&mut self, name: &str) -> BasicBlock<'ctx> {
        self.words.context.append_basic_block(self.function(), name)
    }

    fn set_name(&mut self, value: BasicValueEnum<'ctx>, name: &str) {
        // A constant has no name; only an instruction takes one.
        if let Some(instruction) = value.as_instruction_value() {
            instruction.set_name(name).unwrap();
        }
    }

    fn switch_to_block(&mut self, block: BasicBlock<'ctx>) {
        self.words.builder.position_at_end(block);
    }

    fn entry_slot(&mut self, name: &str) -> PointerValue<'ctx> {
        self.entry_builder()
            .build_alloca(self.words.context.i64_type(), name)
            .unwrap()
    }

    fn entry_slot_array(&mut self, len: u32, name: &str) -> PointerValue<'ctx> {
        let i64_ty = self.words.context.i64_type();
        self.entry_builder()
            .build_array_alloca(i64_ty, i64_ty.const_int(u64::from(len), false), name)
            .unwrap()
    }

    fn load_slot(&mut self, slot: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.words
            .builder
            .build_load(self.words.context.i64_type(), slot, "slot_word")
            .unwrap()
            .into_int_value()
    }

    fn store_slot(&mut self, slot: PointerValue<'ctx>, word: IntValue<'ctx>) {
        self.words.builder.build_store(slot, word).unwrap();
    }

    fn store_slot_at(&mut self, slot: PointerValue<'ctx>, index: u32, word: IntValue<'ctx>) {
        let i64_ty = self.words.context.i64_type();
        // SAFETY: the walker stores only inside the array it allocated.
        let element = unsafe {
            self.words
                .builder
                .build_gep(
                    i64_ty,
                    slot,
                    &[i64_ty.const_int(u64::from(index), false)],
                    &format!("{}_{index}", slot.get_name().to_string_lossy()),
                )
                .unwrap()
        };
        self.words.builder.build_store(element, word).unwrap();
    }

    fn load_slot_at(&mut self, slot: PointerValue<'ctx>, index: u32) -> IntValue<'ctx> {
        let i64_ty = self.words.context.i64_type();
        // SAFETY: the walker loads only inside the array it allocated.
        let element = unsafe {
            self.words
                .builder
                .build_gep(
                    i64_ty,
                    slot,
                    &[i64_ty.const_int(u64::from(index), false)],
                    &format!("{}_{index}", slot.get_name().to_string_lossy()),
                )
                .unwrap()
        };
        self.words
            .builder
            .build_load(i64_ty, element, "slot_word")
            .unwrap()
            .into_int_value()
    }

    fn slot_pointer(&mut self, slot: PointerValue<'ctx>) -> Self::Value {
        slot.into()
    }

    fn bit_constant(&mut self, value: bool) -> IntValue<'ctx> {
        self.words
            .context
            .bool_type()
            .const_int(u64::from(value), false)
    }

    fn bytes_constant(&mut self, bytes: &[u8], prefix: &str, suffix: &str) -> Self::Value {
        let context = self.words.context;
        let array = context.i8_type().array_type(bytes.len() as u32);
        let number = self.state.byte_constants.get();
        self.state.byte_constants.set(number + 1);
        let global =
            self.backend
                .module
                .add_global(array, None, &format!("{prefix}{number}{suffix}"));
        global.set_linkage(inkwell::module::Linkage::Private);
        global.set_initializer(&context.const_string(bytes, false));
        global.set_constant(true);
        global.set_unnamed_addr(true);
        global.as_pointer_value().into()
    }

    fn site_record(&mut self, symbol: &str, image: &[u8]) -> Self::Value {
        self.backend
            .site_record(symbol, image)
            .as_pointer_value()
            .into()
    }

    fn static_cell_block(&mut self, module: &str) -> Self::Value {
        self.backend
            .static_cell_block(module)
            .as_pointer_value()
            .into()
    }

    fn load_word(&mut self, address: Self::Value, offset: i32) -> IntValue<'ctx> {
        let context = self.words.context;
        let builder = self.words.builder;
        // SAFETY: the walker loads only inside the object it names.
        let pointer = unsafe {
            builder
                .build_in_bounds_gep(
                    context.i8_type(),
                    address.into_pointer_value(),
                    &[context.i64_type().const_int(offset as u64, false)],
                    "word_address",
                )
                .unwrap()
        };
        builder
            .build_load(context.i64_type(), pointer, "word")
            .unwrap()
            .into_int_value()
    }

    fn store_word(&mut self, address: Self::Value, offset: i32, word: IntValue<'ctx>) {
        let context = self.words.context;
        let builder = self.words.builder;
        // SAFETY: the walker stores only inside the object it names.
        let pointer = unsafe {
            builder
                .build_in_bounds_gep(
                    context.i8_type(),
                    address.into_pointer_value(),
                    &[context.i64_type().const_int(offset as u64, false)],
                    "word_address",
                )
                .unwrap()
        };
        builder.build_store(pointer, word).unwrap();
    }

    /// An internal function of its own, with a fresh builder and function
    /// state; the module's function order is the order of first requests.
    fn define_helper(
        &mut self,
        name: &str,
        params: usize,
        body: &impl crate::codegen_ssa::HelperBody,
    ) -> FunctionValue<'ctx> {
        use crate::llvm_backend::execution_abi::ExecutionModuleExt;
        if let Some(function) = self.backend.module.get_compiled_function(name) {
            return function;
        }
        let context = self.words.context;
        let i64_type = context.i64_type();
        let function = self.backend.module.add_compiled_function(
            name,
            i64_type.fn_type(&vec![i64_type.into(); params], false),
            Some(inkwell::module::Linkage::Internal),
        );
        let builder = context.create_builder();
        builder.position_at_end(context.append_basic_block(function, "entry"));
        let state = LlvmFunctionState::new(true);
        let mut helper = LlvmCodegen {
            words: LlvmWords {
                context,
                builder: &builder,
            },
            backend: self.backend,
            params: self.params,
            state: &state,
        };
        // Parameter 0 is the execution context.
        let words: Vec<IntValue<'ctx>> = (1..=params as u32)
            .map(|index| function.get_nth_param(index).unwrap().into_int_value())
            .collect();
        let result = body.build(&mut helper, &words);
        builder.build_return(Some(&result)).unwrap();
        function
    }

    fn program_slot(
        &mut self,
        kind: molt_ir::code_slots::ProgramSlotKind,
        owner: &str,
    ) -> IntValue<'ctx> {
        let binding = self.backend.program_slot_binding(kind, owner);
        self.words
            .builder
            .build_load(
                self.words.context.i64_type(),
                binding.as_pointer_value(),
                "program_slot",
            )
            .expect("load immutable program-slot binding")
            .into_int_value()
    }

    fn refcount_site(&mut self, operation: RefcountOperation) -> LlvmRefcountSite<'ctx> {
        self.llvm_refcount_site(operation)
    }

    fn refcount(&mut self, site: LlvmRefcountSite<'ctx>, word: IntValue<'ctx>) {
        site.emit(self.words.builder, word.into());
    }

    /// The admission a retain site reads; it counts as one of the
    /// function's refcount sites, as each site body's retains do.
    fn refcount_admission(&mut self) -> IntValue<'ctx> {
        self.llvm_refcount_site(RefcountOperation::Retain).admission
    }

    /// One always-inlined internal body per site body (`site_helper`), so
    /// the body's blocks never split the caller's block.
    fn site(&mut self, body: SiteBody, args: &[Self::Value]) -> Self::Value {
        let helper = match body {
            SiteBody::FieldGet { offset } => self.backend.field_slot_get_helper(offset),
            SiteBody::FieldSet { offset } => self.backend.field_slot_set_helper(offset),
            SiteBody::ItemGet { containers, cold } => {
                self.backend.list_item_get_helper(containers, cold)
            }
            SiteBody::ListItemSet => self.backend.list_item_set_helper(),
            SiteBody::GlobalSlotRead { index } => self.backend.global_slot_read_helper(index),
            SiteBody::ModuleImportClosed => self.backend.module_import_closed_helper(),
            SiteBody::MethodSiteLoad => self.backend.method_site_load_helper(),
            SiteBody::MethodSiteCall { arguments, cold } => {
                self.backend.method_site_call_helper(arguments, cold)
            }
            SiteBody::StaticCellStore { index } => {
                // The store body returns nothing; its answer is None.
                let helper = self.backend.static_cell_store_helper(index);
                self.call(helper, args);
                return self
                    .words
                    .context
                    .i64_type()
                    .const_int(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE, false)
                    .into();
            }
        };
        self.call(helper, args).expect("a site body answers a word")
    }

    fn placeholder(&mut self, kind: MachineKind) -> Self::Value {
        match self.machine_type(kind) {
            inkwell::types::BasicTypeEnum::IntType(int) => int.get_undef().into(),
            inkwell::types::BasicTypeEnum::FloatType(float) => float.get_undef().into(),
            inkwell::types::BasicTypeEnum::PointerType(pointer) => pointer.get_undef().into(),
            other => unreachable!("{other:?} is no machine kind"),
        }
    }

    fn append_block_param(&mut self, block: BasicBlock<'ctx>, kind: MachineKind) -> Self::Value {
        // Phis lead the block, in parameter order: insert after the last one.
        let builder = self.words.context.create_builder();
        let mut first_other = block.get_first_instruction();
        while let Some(instruction) = first_other
            && instruction.get_opcode() == inkwell::values::InstructionOpcode::Phi
        {
            first_other = instruction.get_next_instruction();
        }
        match first_other {
            Some(instruction) => builder.position_before(&instruction),
            None => builder.position_at_end(block),
        }
        let mut params = self.params.borrow_mut();
        let phis = params.entry(block).or_default();
        let phi = builder
            .build_phi(self.machine_type(kind), &format!("param{}", phis.len()))
            .unwrap();
        phis.push(phi);
        phi.as_basic_value()
    }

    fn jump(&mut self, target: BasicBlock<'ctx>, args: &[Self::Value]) {
        self.pass_arguments(target, args);
        self.words
            .builder
            .build_unconditional_branch(target)
            .unwrap();
    }

    fn branch_if(
        &mut self,
        condition: IntValue<'ctx>,
        yes: (BasicBlock<'ctx>, &[Self::Value]),
        no: (BasicBlock<'ctx>, &[Self::Value]),
    ) {
        self.pass_arguments(yes.0, yes.1);
        self.pass_arguments(no.0, no.1);
        self.words
            .builder
            .build_conditional_branch(condition, yes.0, no.0)
            .unwrap();
    }

    fn switch(
        &mut self,
        selector: IntValue<'ctx>,
        cases: &[(i64, BasicBlock<'ctx>, Vec<Self::Value>)],
        default: (BasicBlock<'ctx>, &[Self::Value]),
    ) {
        self.pass_arguments(default.0, default.1);
        let i64_ty = self.words.context.i64_type();
        let mut targets = Vec::with_capacity(cases.len());
        for (value, target, args) in cases {
            self.pass_arguments(*target, args);
            targets.push((i64_ty.const_int(*value as u64, *value < 0), *target));
        }
        self.words
            .builder
            .build_switch(selector, default.0, &targets)
            .unwrap();
    }

    fn pop_frame(
        &mut self,
        base: Self::Word,
        kind: Option<molt_codegen_abi::frame_record::FrameEntryKind>,
    ) {
        crate::codegen_ssa::frame::leave(self, base, kind);
    }
    fn return_value(&mut self, value: Option<Self::Value>) {
        self.words
            .builder
            .build_return(value.as_ref().map(|v| v as &dyn BasicValue<'ctx>))
            .unwrap();
    }
    fn unreachable(&mut self) {
        self.words.builder.build_unreachable().unwrap();
    }
}
